//! `stepwave install | uninstall`: a per-user Task Scheduler task that starts `stepwave run`
//! at logon and restarts on failure to launch (up to 3 times, a minute apart); crash recovery
//! is done by `stepwave run` itself. No admin rights.

use std::path::Path;

use anyhow::{bail, Context, Result};

/// Task name in Task Scheduler.
pub const TASK_NAME: &str = "stepwave";

/// Task Scheduler XML for `exe run <args>`, scoped to `user` (`DOMAIN\name`). Kept
/// platform-neutral so it can be unit-tested.
pub fn task_xml(exe: &Path, args: &str, user: &str) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let user = escape(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>stepwave footstep enhancer</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>{args}</Arguments></Exec></Actions>
</Task>
"#,
        exe = escape(&exe.display().to_string()),
        args = escape(format!("run {args}").trim_end()),
    )
}

/// UTF-16LE with BOM, the encoding `schtasks /XML` expects.
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn schtasks(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("schtasks")
        .args(args)
        .status()
        .context("running schtasks")?;
    if !status.success() {
        bail!("schtasks {} failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Register (or replace) the logon task for the current user.
pub fn install(run_args: &str) -> Result<()> {
    let exe = std::env::current_exe().context("locating stepwave.exe")?;
    let username = std::env::var("USERNAME").context(
        "USERNAME environment variable is not set; cannot scope the logon task to a user",
    )?;
    let user = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{username}"),
        _ => username,
    };
    let xml = task_xml(&exe, run_args, &user);
    let path = std::env::temp_dir().join("stepwave-task.xml");
    std::fs::write(&path, utf16_with_bom(&xml)).context("writing task XML")?;
    let result = schtasks(&[
        "/Create",
        "/TN",
        TASK_NAME,
        "/XML",
        &path.to_string_lossy(),
        "/F",
    ]);
    let _ = std::fs::remove_file(&path);
    result?;
    println!("installed: stepwave will start at logon (Task Scheduler task '{TASK_NAME}')");
    Ok(())
}

/// Remove the logon task.
pub fn uninstall() -> Result<()> {
    schtasks(&["/Delete", "/TN", TASK_NAME, "/F"])?;
    println!("uninstalled the '{TASK_NAME}' logon task");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_runs_the_exe_with_run_and_escapes() {
        let xml = task_xml(
            Path::new(r"C:\Users\A&B\stepwave.exe"),
            "--mode eq",
            r"DOM\u",
        );
        assert!(xml.contains(r"<Command>C:\Users\A&amp;B\stepwave.exe</Command>"));
        assert!(xml.contains("<Arguments>run --mode eq</Arguments>"));
        assert!(xml.contains("<RestartOnFailure><Interval>PT1M</Interval><Count>3</Count>"));
        assert!(xml.contains("<LogonTrigger>"));
        assert_eq!(xml.matches(r"<UserId>DOM\u</UserId>").count(), 2, "{xml}");
        let bare = task_xml(Path::new("s.exe"), "", "user");
        assert!(bare.contains("<Arguments>run</Arguments>"));
        let escaped = task_xml(Path::new("s.exe"), "", "DOM\\A&B");
        assert!(
            escaped.contains("<UserId>DOM\\A&amp;B</UserId>"),
            "{escaped}"
        );
    }

    #[test]
    fn utf16_has_bom_and_little_endian_units() {
        assert_eq!(utf16_with_bom("A"), vec![0xFF, 0xFE, 0x41, 0x00]);
    }
}
