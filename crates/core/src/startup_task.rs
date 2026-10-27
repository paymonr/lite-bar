//! "Start with Windows": the scheduled task Lite Bar registers (an elevated logon task named
//! `TASK_NAME`), as task XML plus the `schtasks.exe` command lines that create, query and
//! delete it. Pure: the app writes the XML (UTF-16 with a BOM) and runs the commands.

/// The scheduled task's name, in the Task Scheduler's root folder.
pub const TASK_NAME: &str = "Lite Bar";

/// What the Settings window shows for the startup task.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum StartupState {
    /// Not queried yet.
    #[default]
    Unknown,
    /// The task exists and starts this exe.
    On,
    /// No task.
    Off,
    /// The task exists but starts another copy of Lite Bar (the exe was moved).
    OnOtherCopy,
    /// A change or a query is running.
    Busy,
    /// A check or change failed (`msg`, shown in the error color). `actual` is the switch
    /// value checked afterwards, `None` when it couldn't be checked.
    Error { msg: String, actual: Option<bool> },
}

impl StartupState {
    /// The switch value this state verifies: on for `On`, off for `Off` and `OnOtherCopy`
    /// (this copy doesn't start), an error's checked value, and `None` while unknown or busy.
    pub fn actual(&self) -> Option<bool> {
        match self {
            StartupState::On => Some(true),
            StartupState::Off | StartupState::OnOtherCopy => Some(false),
            StartupState::Error { actual, .. } => *actual,
            StartupState::Unknown | StartupState::Busy => None,
        }
    }
}

/// `s` with `& < > " '` escaped for XML text and attribute values.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Undoes `xml_escape`, and numeric character references (`&#38;`, `&#x26;`). Anything
/// else after a `&` is kept as it is.
pub fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest
            .find(';')
            .and_then(|end| entity(&rest[1..end]).map(|c| (c, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character an entity name (between `&` and `;`) stands for.
fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Task Scheduler (schema 1.2) XML for a task that starts `exe_path` elevated (highest
/// available) when `user` (`DOMAIN\user`) signs in: no time limit, runs on battery, and a
/// second start while it runs is ignored.
pub fn task_xml(exe_path: &str, user: &str) -> String {
    let user = xml_escape(user);
    let exe = xml_escape(exe_path);
    // The exe's folder, so relative paths resolve as when it is started by hand.
    let dir = exe_path
        .rfind(['\\', '/'])
        .map(|at| &exe_path[..at])
        .filter(|d| !d.is_empty())
        .map(|d| {
            format!(
                "\n      <WorkingDirectory>{}</WorkingDirectory>",
                xml_escape(d)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts Lite Bar at sign-in</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>{dir}
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `schtasks.exe` arguments that create (or replace) the task from the XML file at `xml_path`.
pub fn create_args(xml_path: &str) -> Vec<String> {
    args(&["/Create", "/TN", TASK_NAME, "/XML", xml_path, "/F"])
}

/// `schtasks.exe` arguments that query the task; exit code 0 means it exists.
pub fn query_args() -> Vec<String> {
    args(&["/Query", "/TN", TASK_NAME])
}

/// `schtasks.exe` arguments that print the task's XML.
pub fn query_xml_args() -> Vec<String> {
    args(&["/Query", "/TN", TASK_NAME, "/XML"])
}

/// `schtasks.exe` arguments that delete the task without asking.
pub fn delete_args() -> Vec<String> {
    args(&["/Delete", "/TN", TASK_NAME, "/F"])
}

/// The program the task's first Exec action runs, unescaped and without quotes.
pub fn task_command(xml: &str) -> Option<String> {
    const OPEN: &str = "<Command>";
    let start = xml.find(OPEN)? + OPEN.len();
    let end = start + xml[start..].find("</Command>")?;
    let cmd = xml_unescape(xml[start..end].trim());
    let cmd = cmd.trim();
    let cmd = cmd
        .strip_prefix('"')
        .and_then(|c| c.strip_suffix('"'))
        .unwrap_or(cmd);
    (!cmd.is_empty()).then(|| cmd.to_string())
}

/// Whether two Windows paths name the same file: no case, either slash.
fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// The state for a task that exists or not, given the XML it printed (if any) and this exe.
/// An existing task whose command can't be read, or a non-ASCII exe path (schtasks prints
/// its XML in the console code page, so it can't be compared reliably), counts as `On`.
pub fn state_from_query(exists: bool, xml: Option<&str>, own_exe: &str) -> StartupState {
    if !exists {
        return StartupState::Off;
    }
    match xml.and_then(task_command) {
        Some(cmd) if own_exe.is_ascii() && !same_path(&cmd, own_exe) => StartupState::OnOtherCopy,
        _ => StartupState::On,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Games & Tools\Lite Bar\lite-bar.exe";
    const USER: &str = r"DESKTOP-1\paymon";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The text of the first `<tag>` element.
    fn element<'a>(xml: &'a str, tag: &str) -> &'a str {
        let open = format!("<{tag}>");
        let start = xml.find(&open).unwrap_or_else(|| panic!("no <{tag}>")) + open.len();
        let end = xml[start..].find(&format!("</{tag}>")).unwrap() + start;
        &xml[start..end]
    }

    #[test]
    fn escapes_all_five_characters() {
        assert_eq!(
            xml_escape(r#"a&b<c>d"e'f"#),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
        assert_eq!(xml_escape(r"C:\plain path\x.exe"), r"C:\plain path\x.exe");
    }

    #[test]
    fn unescape_reverses_escape_and_numeric_references() {
        let raw = r#"C:\a & b\<x> "q" 'y'.exe"#;
        assert_eq!(xml_unescape(&xml_escape(raw)), raw);
        assert_eq!(xml_unescape("a&#38;b&#x26;c&#X26;d"), "a&b&c&d");
        // Not an entity: kept as is.
        assert_eq!(xml_unescape("a&b &unknown; c&"), "a&b &unknown; c&");
    }

    #[test]
    fn xml_has_the_required_elements() {
        let x = task_xml(EXE, USER);
        assert!(x.starts_with(r#"<?xml version="1.0" encoding="UTF-16"?>"#));
        assert!(x.contains(
            r#"<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">"#
        ));
        assert_eq!(element(&x, "Description"), "Starts Lite Bar at sign-in");
        let trigger = element(&x, "LogonTrigger");
        assert_eq!(element(trigger, "UserId"), USER);
        assert_eq!(element(trigger, "Enabled"), "true");
        let principal = element(&x, "Principals");
        assert_eq!(element(principal, "UserId"), USER);
        assert_eq!(element(principal, "LogonType"), "InteractiveToken");
        assert_eq!(element(principal, "RunLevel"), "HighestAvailable");
        let settings = element(&x, "Settings");
        assert_eq!(element(settings, "MultipleInstancesPolicy"), "IgnoreNew");
        assert_eq!(element(settings, "DisallowStartIfOnBatteries"), "false");
        assert_eq!(element(settings, "StopIfGoingOnBatteries"), "false");
        assert_eq!(element(settings, "ExecutionTimeLimit"), "PT0S");
        assert_eq!(element(settings, "Priority"), "7");
        assert_eq!(element(settings, "StartWhenAvailable"), "false");
        assert_eq!(element(settings, "AllowHardTerminate"), "true");
        assert!(x.contains(r#"<Actions Context="Author">"#));
        assert!(x.contains(r#"<Principal id="Author">"#));
    }

    #[test]
    fn exe_path_with_an_ampersand_and_spaces_is_escaped() {
        let x = task_xml(EXE, USER);
        assert_eq!(
            element(element(&x, "Exec"), "Command"),
            r"C:\Games &amp; Tools\Lite Bar\lite-bar.exe"
        );
        assert!(!x.contains("Games & Tools"));
        assert_eq!(task_command(&x).as_deref(), Some(EXE));
    }

    #[test]
    fn starts_in_the_exe_folder() {
        let x = task_xml(EXE, USER);
        assert_eq!(
            element(element(&x, "Exec"), "WorkingDirectory"),
            r"C:\Games &amp; Tools\Lite Bar"
        );
        let x = task_xml("C:/x/lite-bar.exe", USER);
        assert_eq!(element(element(&x, "Exec"), "WorkingDirectory"), "C:/x");
        // No folder in the path: none in the task either.
        assert!(!task_xml("lite-bar.exe", USER).contains("WorkingDirectory"));
    }

    #[test]
    fn actual_is_the_verified_switch_value() {
        let err = |actual| StartupState::Error {
            msg: "failed".into(),
            actual,
        };
        assert_eq!(StartupState::On.actual(), Some(true));
        assert_eq!(StartupState::Off.actual(), Some(false));
        assert_eq!(StartupState::OnOtherCopy.actual(), Some(false));
        assert_eq!(StartupState::Unknown.actual(), None);
        assert_eq!(StartupState::Busy.actual(), None);
        assert_eq!(err(Some(true)).actual(), Some(true));
        assert_eq!(err(None).actual(), None);
    }

    #[test]
    fn user_is_escaped_too() {
        let x = task_xml(EXE, r"DOM\o'brien&co");
        assert_eq!(
            element(element(&x, "LogonTrigger"), "UserId"),
            r"DOM\o&apos;brien&amp;co"
        );
    }

    #[test]
    fn schtasks_argument_lists() {
        assert_eq!(
            create_args(r"C:\Temp\t.xml"),
            s(&["/Create", "/TN", "Lite Bar", "/XML", r"C:\Temp\t.xml", "/F"])
        );
        assert_eq!(query_args(), s(&["/Query", "/TN", "Lite Bar"]));
        assert_eq!(query_xml_args(), s(&["/Query", "/TN", "Lite Bar", "/XML"]));
        assert_eq!(delete_args(), s(&["/Delete", "/TN", "Lite Bar", "/F"]));
    }

    #[test]
    fn task_command_reads_what_schtasks_prints() {
        // schtasks /Query /XML output: CR CR LF line ends, indentation, a quoted command.
        let printed = "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\r\n<Task>\r\r\n  <Actions Context=\"Author\">\r\r\n    <Exec>\r\r\n      <Command>\"C:\\Lite Bar\\lite-bar.exe\"</Command>\r\r\n    </Exec>\r\r\n  </Actions>\r\r\n</Task>";
        assert_eq!(
            task_command(printed).as_deref(),
            Some(r"C:\Lite Bar\lite-bar.exe")
        );
        assert_eq!(task_command("<Task></Task>"), None);
    }

    #[test]
    fn query_states() {
        let mine = task_xml(EXE, USER);
        let other = task_xml(r"D:\old\lite-bar.exe", USER);
        assert_eq!(state_from_query(false, None, EXE), StartupState::Off);
        assert_eq!(state_from_query(true, Some(&mine), EXE), StartupState::On);
        // Windows paths compare without case and with either slash.
        let upper = EXE.to_uppercase().replace('\\', "/");
        assert_eq!(
            state_from_query(true, Some(&mine), &upper),
            StartupState::On
        );
        assert_eq!(
            state_from_query(true, Some(&other), EXE),
            StartupState::OnOtherCopy
        );
        // Can't tell where it points: on.
        assert_eq!(state_from_query(true, None, EXE), StartupState::On);
        assert_eq!(
            state_from_query(true, Some("garbage"), EXE),
            StartupState::On
        );
        assert_eq!(
            state_from_query(true, Some(&other), r"C:\Spiele\Ü\lite-bar.exe"),
            StartupState::On
        );
    }
}
