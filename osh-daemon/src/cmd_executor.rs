//! Simple command executor (non-PTY mode)
//!
//! Executes commands without PTY, returns stdout/stderr directly.

use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Events from command execution
#[derive(Debug)]
pub enum CmdEvent {
    /// Output data from process (stdout or stderr combined)
    Data { req_id: String, data: Vec<u8> },
    /// Process exited
    Exit { req_id: String, exit_code: i32 },
    /// Error occurred
    Error { req_id: String, error: String },
}

/// Execute a command without PTY
pub async fn execute_command(
    shell: &str,
    req_id: String,
    command: &str,
    event_tx: mpsc::Sender<CmdEvent>,
) -> Result<()> {
    let shell = shell.to_string();
    let command = command.to_string();
    
    tokio::spawn(async move {
        let result = run_command(&shell, &command).await;
        
        match result {
            Ok((stdout, stderr, exit_code)) => {
                // Send stderr first if any
                if !stderr.is_empty() {
                    let _ = event_tx.send(CmdEvent::Data {
                        req_id: req_id.clone(),
                        data: stderr,
                    }).await;
                }
                
                // Send stdout
                if !stdout.is_empty() {
                    let _ = event_tx.send(CmdEvent::Data {
                        req_id: req_id.clone(),
                        data: stdout,
                    }).await;
                }
                
                // Send exit
                let _ = event_tx.send(CmdEvent::Exit {
                    req_id,
                    exit_code,
                }).await;
            }
            Err(e) => {
                error!("Command execution failed: {}", e);
                let _ = event_tx.send(CmdEvent::Error {
                    req_id,
                    error: e.to_string(),
                }).await;
            }
        }
    });
    
    Ok(())
}

/// Run a command and capture output
async fn run_command(shell: &str, command: &str) -> Result<(Vec<u8>, Vec<u8>, i32)> {
    let mut cmd = Command::new(shell);
    
    #[cfg(target_os = "windows")]
    {
        if shell.to_lowercase().contains("powershell") {
            cmd.arg("-NoLogo");
            cmd.arg("-NoProfile");
            cmd.arg("-NonInteractive");
            cmd.arg("-Command");
            cmd.arg(command);
        } else {
            // cmd.exe
            cmd.arg("/C");
            cmd.arg(command);
        }
    }
    
    #[cfg(not(target_os = "windows"))]
    {
        cmd.arg("-c");
        cmd.arg(command);
    }
    
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    
    debug!("Executing: {} [command: {}]", shell, command);
    
    let output = cmd.output().await
        .context("Failed to execute command")?;
    
    let exit_code = output.status.code().unwrap_or(-1);
    
    info!("Command finished with exit code: {}", exit_code);
    
    Ok((output.stdout, output.stderr, exit_code))
}
