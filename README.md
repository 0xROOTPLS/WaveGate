# WaveGate
WaveGate is an "advanced", visually appealing remote access tool with a plethora of features for the operator.

# Disclaimer
- Wavegate is in very early stages of development (1.0) - Please expect potentialy catestrophic bugs. I did my best to ensure each module works, but the codebase got so large its hard to keep all the gears working together.
- Wavegate clients were not designed to be evasive. It is probably very signaturable.
- Wavegate was made for the community, and for those who want a great resource for offensive Rust-based RAT features.

# Features
- **Remote Shell**: Interactive streamed shell sessions with input/output forwarding.
- **Remote Execution**: Including web downloads and local file execution, with output capture.
- **File Manager**: Full custom interactive Windows file manager with browsing, upload/download, delete, rename, and directory creation capabilities.
- **Registry Editor**: Full custom registry editor with key/value listing, getting/setting/deleting values, creating/deleting keys (with recursive delete option).
- **Process Manager**: Process listing and killing.
- **Startup Manager**: Management of startup items, including adding/removing entries.
- **Services Manager**: Service listing, starting, stopping, and configuration.
- **Task Scheduler Manager**: Task listing, creation, deletion, and execution.
- **Clipboard Manager**: Clipboard monitoring with history, content retrieval, setting, and regex-based replacement.
- **WMI Console**: WMI query execution with pre-built commands for system information and management.
- **Remote Desktop**: Interactive remote desktop with mouse/keyboard control, supporting H.264 encoding, tile-based DXGI capture at up to 30 FPS, or JPEG BitBlt fallback.
- **Remote Webcam/Media Stream**: Remote webcam and audio streaming with configurable video/audio devices, FPS, quality, and resolution; uses direct MPEG encoding.
- **Screenshot**: On-demand screenshot capture.
- **GUI User Chat**: Interactive chat window for communicating with the end user, with event polling and forwarding.
- **Open URL**: Ability to open specified URLs in the default browser.
- **Credential Recovery**: Recovery of stored credentials, browser cookies (Chromium v10, v11, and v20).
- **TCP Connections Manager**: Listing and management of active TCP connections.
- **Hosts File Manager**: Editing and management of the hosts file for DNS redirection.
- **Cached DNS Manager/Poisoner**: DNS cache management, including poisoning for MITM attacks.
- **Lateral Movement Module**: Token creation/impersonation; jumping to remote hosts via SCShell, PExec, and SMB pipes; remote execution via WMI, WinRM, and SMB; pivoting capabilities.
- **Active Directory Enumeration**: Full AD enumeration including users, machines, groups, domain info, and trusts.
- **Kerberos Module**: Listing, purging, and enumerating Kerberos tickets; requesting and managing service tickets.
- **Reverse Proxy**: SOCKS5-style reverse proxy for tunneling, with support for SMB/named pipe proxies; handles connect, data, and close messages.
- **Primary/Backup Hosts**: Connection attempts to primary host first, falling back to backup if configured.
- **Domain Fronting**: Supported via custom SNI hostname overriding.
- **Custom DNS Support**: System DNS or custom primary/backup DNS servers for resolution, with manual query handling.
- **UAC Bypass**: Built-in UAC bypass for elevation, with relaunch on demand.
- **Proxy Aware**: Supports HTTP or SOCKS5 proxies for outbound connections, with optional username/password authentication.
- **Persistence Methods**: 4 methods including registry run keys, task scheduler, startup folder, and service installation.
- **Zone ID Clearing**: Automatic clearing of Zone.Identifier ADS to bypass MOTW.
- **Auto System Sleep Prevention**: Configurable prevention of system sleep/idle.
- **Startup/Connect/Reconnect Delays**: Configurable delays for run start, initial connect, and reconnect attempts.
- **Auto Uninstall Triggers**: Triggers based on hostname, date, environment, with full cleanup.
- **Anti-VM Detection**: Detection via WMI queries to exit if in VM.
- **Configuration Security**: Config is encrypted and brute-forced at runtime (no key stored), then zeroed out in memory.
- **Communication Modes**: Regular TLS stream or HTTP upgrade to WebSocket for comms, with framing support.
- **Geolocation Fetching**: One-time geolocation fetch from ip-api.com at startup, cached for system info.
- **Single-Instance Enforcement**: Uses named mutex and lock file to ensure only one instance runs, with retry for restarts.
- **Disclosure Dialog**: Optional user disclosure dialog at startup, exiting if declined.
- **VM/Environment Checks**: Additional environment checks beyond anti-VM, including install location enforcement.
- **System Info Gathering**: Comprehensive info including hardware (CPU, GPU, RAM, motherboard, drives via WMI and sysinfo), usage (CPU/RAM percent), network (IPs, country), and real-time updates (active window, uptime).
- **Persistent UID**: Generated from machine GUID + build ID for unique identification.
- **Protocol Handling**: Binary protocol with length-prefixing, message types (register, ping/pong, commands, responses, info updates, media/RD frames, proxy messages), and WebSocket wrapping.
- **Panic Protection**: Command execution wrapped in panic catching for stability.
- **Cleanup on Disconnect**: Automatic cleanup of active sessions (shell, media, RD, proxy) on disconnect.

# Images
### Main Client Area
![Main Client Area](https://files.catbox.moe/swnbaa.PNG)

### Context Menu
![Context Menu](https://files.catbox.moe/yxwlie.PNG)

### File Manager
![File Manager](https://files.catbox.moe/q8v91e.PNG)

### Remote Shell
![Remote Shell](https://files.catbox.moe/v4n0nc.PNG)

### Registry Editor
![Registry Editor](https://files.catbox.moe/xf1tix.PNG)

### WMI Console
![WMI Console](https://files.catbox.moe/18o1wd.PNG)

### User Chat
![User Chat](https://files.catbox.moe/x838s5.PNG)

### Credential Recovery
![Credential Recovery](https://files.catbox.moe/enz4as.PNG)

### Lateral Movement — Tokens
![Lateral Movement — Tokens](https://files.catbox.moe/wa7z0o.PNG)

### Lateral Movement — Jump
![Lateral Movement — Jump](https://files.catbox.moe/fothyy.PNG)

### Lateral Movement — Pivots
![Lateral Movement — Pivots](https://files.catbox.moe/vcynd6.PNG)

### Lateral Movement — Remote Exec
![Lateral Movement — Remote Exec](https://files.catbox.moe/ai9zxa.PNG)

### Lateral Movement — AD
![Lateral Movement — AD](https://files.catbox.moe/tu1nzv.PNG)

### Lateral Movement — Kerberos
![Lateral Movement — Kerberos](https://files.catbox.moe/crn9oc.PNG)

### Builder Part 1
![Builder Part 1](https://files.catbox.moe/fd76hr.PNG)

### Builder Part 2
![Builder Part 2](https://files.catbox.moe/uof0wp.PNG)

### Server Config
![Server Config](https://files.catbox.moe/titc84.PNG)

### Logs
![Logs](https://files.catbox.moe/ofw8lk.PNG)

### General Settings 1
![General Settings 1](https://files.catbox.moe/0c8i5i.PNG)

### General Settings 2
![General Settings 2](https://files.catbox.moe/k017ul.PNG)
