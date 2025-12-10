//! Windows Task Scheduler Manager for the client.
//!
//! Enumerates scheduled tasks and provides control operations (run, enable, disable, delete, create).

use wavegate_shared::{CommandResponseData, ScheduledTaskInfo};

/// List all scheduled tasks
pub fn list_scheduled_tasks() -> (bool, CommandResponseData) {
    match get_scheduled_tasks() {
        Ok(tasks) => (true, CommandResponseData::ScheduledTaskList { tasks }),
        Err(e) => (false, CommandResponseData::Error { message: e }),
    }
}

/// Run a scheduled task immediately
pub fn run_task(task_name: &str) -> (bool, CommandResponseData) {
    match execute_task_action(task_name, TaskAction::Run) {
        Ok(msg) => (true, CommandResponseData::ScheduledTaskResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ScheduledTaskResult { success: false, message: e }),
    }
}

/// Enable a scheduled task
pub fn enable_task(task_name: &str) -> (bool, CommandResponseData) {
    match execute_task_action(task_name, TaskAction::Enable) {
        Ok(msg) => (true, CommandResponseData::ScheduledTaskResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ScheduledTaskResult { success: false, message: e }),
    }
}

/// Disable a scheduled task
pub fn disable_task(task_name: &str) -> (bool, CommandResponseData) {
    match execute_task_action(task_name, TaskAction::Disable) {
        Ok(msg) => (true, CommandResponseData::ScheduledTaskResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ScheduledTaskResult { success: false, message: e }),
    }
}

/// Delete a scheduled task
pub fn delete_task(task_name: &str) -> (bool, CommandResponseData) {
    match execute_task_action(task_name, TaskAction::Delete) {
        Ok(msg) => (true, CommandResponseData::ScheduledTaskResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ScheduledTaskResult { success: false, message: e }),
    }
}

/// Create a new scheduled task
pub fn create_task(
    name: &str,
    description: Option<&str>,
    action_path: &str,
    action_args: Option<&str>,
    trigger_type: &str,
    start_time: Option<&str>,
    interval: Option<u32>,
) -> (bool, CommandResponseData) {
    match create_scheduled_task(name, description, action_path, action_args, trigger_type, start_time, interval) {
        Ok(msg) => (true, CommandResponseData::ScheduledTaskResult { success: true, message: msg }),
        Err(e) => (false, CommandResponseData::ScheduledTaskResult { success: false, message: e }),
    }
}

enum TaskAction {
    Run,
    Enable,
    Disable,
    Delete,
}

/// Get all scheduled tasks using schtasks.exe (simpler and more reliable than COM)
fn get_scheduled_tasks() -> Result<Vec<ScheduledTaskInfo>, String> {
    use std::process::Command;

    // Use schtasks.exe to get task list in CSV format
    let output = Command::new("schtasks.exe")
        .args(["/Query", "/FO", "CSV", "/V"])
        .output()
        .map_err(|e| format!("Failed to run schtasks: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("schtasks failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tasks = Vec::new();
    let mut lines = stdout.lines();

    // Skip header line
    let header = lines.next().unwrap_or("");
    let headers: Vec<&str> = parse_csv_line(header);

    // Find column indices
    let hostname_idx = headers.iter().position(|h| h.contains("HostName")).unwrap_or(0);
    let taskname_idx = headers.iter().position(|h| h.contains("TaskName")).unwrap_or(1);
    let next_run_idx = headers.iter().position(|h| h.contains("Next Run Time")).unwrap_or(2);
    let status_idx = headers.iter().position(|h| h.contains("Status")).unwrap_or(3);
    let last_run_idx = headers.iter().position(|h| h.contains("Last Run Time")).unwrap_or(5);
    let last_result_idx = headers.iter().position(|h| h.contains("Last Result")).unwrap_or(6);
    let task_to_run_idx = headers.iter().position(|h| h.contains("Task To Run")).unwrap_or(8);
    let schedule_type_idx = headers.iter().position(|h| h.contains("Schedule Type")).unwrap_or(12);

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = parse_csv_line(line);
        if fields.len() <= taskname_idx {
            continue;
        }

        let full_name = fields.get(taskname_idx).unwrap_or(&"").to_string();

        // Skip if empty
        if full_name.is_empty() || full_name == "TaskName" {
            continue;
        }

        // Parse path and name
        let (path, name) = if let Some(idx) = full_name.rfind('\\') {
            (full_name[..idx].to_string(), full_name[idx+1..].to_string())
        } else {
            ("\\".to_string(), full_name.clone())
        };

        let status = fields.get(status_idx).unwrap_or(&"Unknown").to_string();
        let last_run = fields.get(last_run_idx).unwrap_or(&"Never").to_string();
        let next_run = fields.get(next_run_idx).unwrap_or(&"None").to_string();
        let action = fields.get(task_to_run_idx).unwrap_or(&"").to_string();
        let trigger = fields.get(schedule_type_idx).unwrap_or(&"").to_string();

        // Parse last result as integer
        let last_result_str = fields.get(last_result_idx).unwrap_or(&"0");
        let last_result = last_result_str.parse::<i32>().unwrap_or(0);

        tasks.push(ScheduledTaskInfo {
            name,
            path,
            status,
            last_run,
            last_result,
            next_run,
            trigger,
            action,
        });
    }

    // Sort by name
    tasks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(tasks)
}

/// Parse a CSV line handling quoted fields
fn parse_csv_line(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut in_quotes = false;
    let mut field_start = 0;
    let bytes = line.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if b == b',' && !in_quotes {
            let field = &line[field_start..i];
            // Remove surrounding quotes if present
            let field = field.trim_matches('"');
            fields.push(field);
            field_start = i + 1;
        }
    }

    // Add the last field
    if field_start < line.len() {
        let field = &line[field_start..];
        let field = field.trim_matches('"');
        fields.push(field);
    }

    fields
}

/// Execute a task action using schtasks.exe
fn execute_task_action(task_name: &str, action: TaskAction) -> Result<String, String> {
    use std::process::Command;

    let args: Vec<&str> = match action {
        TaskAction::Run => vec!["/Run", "/TN", task_name],
        TaskAction::Enable => vec!["/Change", "/TN", task_name, "/Enable"],
        TaskAction::Disable => vec!["/Change", "/TN", task_name, "/Disable"],
        TaskAction::Delete => vec!["/Delete", "/TN", task_name, "/F"],
    };

    let output = Command::new("schtasks.exe")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run schtasks: {}", e))?;

    if output.status.success() {
        let action_name = match action {
            TaskAction::Run => "started",
            TaskAction::Enable => "enabled",
            TaskAction::Disable => "disabled",
            TaskAction::Delete => "deleted",
        };
        Ok(format!("Task '{}' {}", task_name, action_name))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!("Failed: {} {}", stdout, stderr).trim().to_string())
    }
}

/// Create a new scheduled task using schtasks.exe
fn create_scheduled_task(
    name: &str,
    description: Option<&str>,
    action_path: &str,
    action_args: Option<&str>,
    trigger_type: &str,
    start_time: Option<&str>,
    interval: Option<u32>,
) -> Result<String, String> {
    use std::process::Command;

    let mut args = vec!["/Create", "/TN", name, "/TR"];

    // Build the action string
    let action = if let Some(args_str) = action_args {
        format!("\"{}\" {}", action_path, args_str)
    } else {
        format!("\"{}\"", action_path)
    };
    args.push(&action);

    // Add schedule type
    let sc_type = match trigger_type.to_lowercase().as_str() {
        "once" => "ONCE",
        "daily" => "DAILY",
        "weekly" => "WEEKLY",
        "monthly" => "MONTHLY",
        "logon" => "ONLOGON",
        "startup" => "ONSTART",
        "idle" => "ONIDLE",
        _ => "ONCE",
    };
    args.push("/SC");
    args.push(sc_type);

    // Add start time for time-based triggers
    let time_str;
    if let Some(st) = start_time {
        // Extract time portion (HH:MM) from ISO format or use as-is
        time_str = if st.contains('T') {
            st.split('T').nth(1).unwrap_or("00:00").split(':').take(2).collect::<Vec<_>>().join(":")
        } else {
            st.to_string()
        };
        args.push("/ST");
        args.push(&time_str);
    }

    // Add interval/modifier for daily/weekly
    let interval_str;
    if let Some(int) = interval {
        if trigger_type == "daily" || trigger_type == "weekly" {
            interval_str = int.to_string();
            args.push("/MO");
            args.push(&interval_str);
        }
    }

    // Force overwrite if exists
    args.push("/F");

    let output = Command::new("schtasks.exe")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run schtasks: {}", e))?;

    if output.status.success() {
        Ok(format!("Task '{}' created successfully", name))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!("Failed to create task: {} {}", stdout, stderr).trim().to_string())
    }
}
