// Permission for the mouse shortcut, asked of the user and never taken: a udev rule file of
// Lookitan's own, written or removed by one fixed root script through polkit (pkexec), only when
// the user clicks the button in Configurações › Atalhos.
//
// What the rule gives is udev's `uaccess`: the devices belong to whoever sits at this computer
// (the active local session), not to a group and not to everybody, and logind takes them back at
// logout or when another user takes the seat. Nobody is added to the `input` group, no device
// mode changes, and Lookitan never runs as root: the only thing root runs is `SCRIPT` below, with
// the rule's text as its one argument (set by Lookitan before polkit asks, shown in its dialog,
// and not something another program can swap afterwards, as a temporary file would be).

use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

pub const RULE_PATH: &str = "/etc/udev/rules.d/70-lookitan-input.rules";
/// The first line of the file: only a file that starts with it is replaced or removed.
const MARK: &str = "# Lookitan: mouse-button shortcuts";

/// The parts of the rule beyond reading the mice, each the user's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Keyboards may be asked whether Ctrl, Alt or Shift are down. The same access lets any
    /// program of the user read everything typed, which Wayland otherwise prevents.
    pub keyboard: bool,
    /// /dev/uinput, for the copy that stands in for a grabbed mouse (blocking the click). The
    /// same access lets any program of the user send keys and clicks as if typed.
    pub uinput: bool,
}

/// The rule file. Numbered 70 so that it runs after 60-input-id (which says what is a mouse) and
/// before 73-seat-late (which hands the tagged devices to the session).
pub fn rule_text(rule: Rule) -> String {
    let mut text = format!(
        "{MARK}\n\
         # Written from Lookitan's settings (Configurações › Atalhos), which also removes it. By hand:\n\
         # delete this file, then run: udevadm control --reload\n\
         # \"uaccess\" gives a device to whoever is at this computer's screen and keyboard (the active\n\
         # local session) and takes it back when they log out. No group or file mode is changed.\n\
         \n\
         # Mice, trackballs and pointing sticks: Lookitan reads their buttons.\n\
         SUBSYSTEM==\"input\", KERNEL==\"event*\", ENV{{ID_INPUT_MOUSE}}==\"1\", TAG+=\"uaccess\"\n"
    );
    if rule.keyboard {
        text.push_str(
            "\n# Keyboards: Lookitan asks whether Ctrl, Alt or Shift are down when the shortcut's button is\n\
             # pressed. It never reads what is typed, but this access would let any of your programs do so.\n\
             SUBSYSTEM==\"input\", KERNEL==\"event*\", ENV{ID_INPUT_KEYBOARD}==\"1\", TAG+=\"uaccess\"\n",
        );
    }
    if rule.uinput {
        text.push_str(
            "\n# Virtual input devices: to keep the shortcut's click from the program in front, Lookitan takes\n\
             # the mouse for itself and passes everything else on through a copy of it. This access would\n\
             # let any of your programs send keys and clicks.\n\
             KERNEL==\"uinput\", SUBSYSTEM==\"misc\", TAG+=\"uaccess\", OPTIONS+=\"static_node=uinput\"\n\
             # The copy gets the mouse's own entry of the hardware database (its DPI), as the mouse does.\n\
             SUBSYSTEM==\"input\", KERNEL==\"event*\", ATTRS{phys}==\"lookitan/mouse-hotkey\", ATTRS{id/bustype}==\"0003\", \
             IMPORT{builtin}=\"hwdb 'mouse:usb:v$attr{id/vendor}p$attr{id/product}:name:$attr{name}:'\"\n\
             SUBSYSTEM==\"input\", KERNEL==\"event*\", ATTRS{phys}==\"lookitan/mouse-hotkey\", ATTRS{id/bustype}==\"0005\", \
             IMPORT{builtin}=\"hwdb 'mouse:bluetooth:v$attr{id/vendor}p$attr{id/product}:name:$attr{name}:'\"\n",
        );
    }
    text
}

/// Which parts a rule file of Lookitan's has; None for a file that is not Lookitan's.
pub fn parse(text: &str) -> Option<Rule> {
    if !text.starts_with(MARK) {
        return None;
    }
    let has = |needle: &str| text.lines().any(|line| !line.trim_start().starts_with('#') && line.contains(needle));
    Some(Rule { keyboard: has("ID_INPUT_KEYBOARD"), uinput: has("KERNEL==\"uinput\"") })
}

/// The rule installed now, if it is Lookitan's.
pub fn installed() -> Option<Rule> {
    parse(&std::fs::read_to_string(RULE_PATH).ok()?)
}

/// What root runs, whole. `$1` is the new rule file's text, or nothing to remove the file.
/// - A file there that Lookitan did not write is left alone.
/// - Access a part of the old rule gave and the new one does not is taken back at once from the
///   devices connected now (logind would only do it at the next login); if another rule still
///   gives it, the trigger at the end gives it back.
/// - Nothing else is read, written or run: no path, name or command comes from outside.
const SCRIPT: &str = r#"set -eu
f=/etc/udev/rules.d/70-lookitan-input.rules
had=0; had_keyboard=0; had_uinput=0
if [ -e "$f" ]; then
    if ! grep -q '^# Lookitan: mouse-button shortcuts' "$f"; then
        echo "lookitan: $f was not written by Lookitan; nothing was changed" >&2
        exit 3
    fi
    had=1
    grep -q '^SUBSYSTEM.*ID_INPUT_KEYBOARD' "$f" && had_keyboard=1
    grep -q '^KERNEL=="uinput"' "$f" && had_uinput=1
fi
if [ -n "$1" ]; then
    umask 022
    printf '%s' "$1" > "$f.new"
    mv -f "$f.new" "$f"
else
    rm -f "$f"
fi
udevadm control --reload
drop_mouse=0; drop_keyboard=0; drop_uinput=0
[ "$had" = 1 ] && [ -z "$1" ] && drop_mouse=1
case "$1" in *ID_INPUT_KEYBOARD*) ;; *) drop_keyboard=$had_keyboard ;; esac
case "$1" in *'KERNEL=="uinput"'*) modprobe uinput 2>/dev/null || true ;; *) drop_uinput=$had_uinput ;; esac
uid=${PKEXEC_UID:-}
case "$uid" in ''|*[!0-9]*) uid= ;; esac
if [ -n "$uid" ] && command -v setfacl >/dev/null 2>&1; then
    for n in /dev/input/event*; do
        [ -e "$n" ] || continue
        p=$(udevadm info --query=property --name="$n" 2>/dev/null || true)
        case "$p" in *ID_INPUT_MOUSE=1*) [ "$drop_mouse" = 1 ] && setfacl -x "u:$uid" "$n" 2>/dev/null || true ;; esac
        case "$p" in *ID_INPUT_KEYBOARD=1*) [ "$drop_keyboard" = 1 ] && setfacl -x "u:$uid" "$n" 2>/dev/null || true ;; esac
    done
    [ "$drop_uinput" = 1 ] && [ -e /dev/uinput ] && setfacl -x "u:$uid" /dev/uinput 2>/dev/null || true
fi
udevadm trigger --action=change --subsystem-match=input
udevadm trigger --action=change --subsystem-match=misc --sysname-match=uinput 2>/dev/null || true
udevadm settle --timeout=5 2>/dev/null || true
"#;

/// Whether polkit's pkexec is there to ask for the administrator's password.
pub fn can_elevate() -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("pkexec").is_file()))
}

/// The same change for a terminal, for a system without polkit or without its password dialog.
pub fn manual(rule: Option<Rule>) -> String {
    match rule {
        Some(rule) => format!(
            "sudo tee {RULE_PATH} >/dev/null <<'EOF'\n{}EOF\nsudo udevadm control --reload\nsudo udevadm trigger --action=change --subsystem-match=input\n{}",
            rule_text(rule),
            if rule.uinput { "sudo modprobe uinput\nsudo udevadm trigger --action=change --subsystem-match=misc --sysname-match=uinput\n" } else { "" },
        ),
        None => format!("sudo rm -f {RULE_PATH}\nsudo udevadm control --reload\n# O acesso já dado vale até o fim da sessão.\n"),
    }
}

/// Writes the rule (`Some`) or removes it (`None`), after polkit asked for the password. Blocks
/// while its dialog is open.
pub fn apply(rule: Option<Rule>) -> Result<(), String> {
    if !can_elevate() {
        return Err("O pkexec (polkit) não está instalado. Use os comandos manuais abaixo.".into());
    }
    let text = rule.map(rule_text).unwrap_or_default();
    let output = Command::new("pkexec")
        .args(["/bin/sh", "-c", SCRIPT, "lookitan-input-rule", text.as_str()])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Não foi possível chamar o pkexec: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(match output.status.code() {
        Some(126) => "A autorização foi cancelada; nada foi alterado.".to_string(),
        Some(127) => "O polkit não pôde pedir a senha (sem agente de autenticação nesta sessão?). Use os comandos manuais abaixo.".to_string(),
        Some(3) => format!("{RULE_PATH} existe e não foi escrito pelo Lookitan; nada foi alterado."),
        code => format!("A alteração falhou (código {}): {}", code.map_or("?".to_string(), |c| c.to_string()), stderr.trim()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_gives_only_the_parts_asked_for() {
        let mice = rule_text(Rule::default());
        assert!(mice.starts_with(MARK));
        assert!(mice.contains("ENV{ID_INPUT_MOUSE}==\"1\", TAG+=\"uaccess\""));
        assert!(!mice.contains("ID_INPUT_KEYBOARD") && !mice.contains("uinput"), "{mice}");
        let all = rule_text(Rule { keyboard: true, uinput: true });
        assert!(all.contains("ENV{ID_INPUT_KEYBOARD}==\"1\", TAG+=\"uaccess\""));
        assert!(all.contains("KERNEL==\"uinput\", SUBSYSTEM==\"misc\", TAG+=\"uaccess\", OPTIONS+=\"static_node=uinput\""));
        assert!(all.contains("'mouse:usb:v$attr{id/vendor}p$attr{id/product}:name:$attr{name}:'"));
    }

    #[test]
    fn the_rule_never_opens_devices_to_everybody() {
        for rule in [Rule::default(), Rule { keyboard: true, uinput: false }, Rule { keyboard: true, uinput: true }] {
            let text = rule_text(rule);
            for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
                assert!(!line.contains("MODE") && !line.contains("GROUP") && !line.contains("OWNER") && !line.contains("RUN"), "{line}");
                assert!(line.contains("TAG+=\"uaccess\"") || line.contains("IMPORT{builtin}=\"hwdb"), "{line}");
            }
        }
        for forbidden in ["usermod", "gpasswd", "chmod", "chown", "chgrp", "eval", "$2"] {
            assert!(!SCRIPT.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn an_installed_rule_is_read_back_and_a_stranger_s_file_is_not_lookitan_s() {
        for rule in [Rule::default(), Rule { keyboard: true, uinput: false }, Rule { keyboard: false, uinput: true }, Rule { keyboard: true, uinput: true }] {
            assert_eq!(parse(&rule_text(rule)), Some(rule));
        }
        assert_eq!(parse("KERNEL==\"uinput\", MODE=\"0666\"\n"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn the_script_guards_the_file_it_replaces_with_the_rule_s_own_first_line() {
        assert!(SCRIPT.contains(&format!("'^{MARK}'")));
        assert!(SCRIPT.contains(&format!("f={RULE_PATH}\n")));
        // The checks for the old rule's parts match the lines `rule_text` writes.
        let all = rule_text(Rule { keyboard: true, uinput: true });
        assert!(all.lines().any(|l| l.starts_with("SUBSYSTEM") && l.contains("ID_INPUT_KEYBOARD")));
        assert!(all.lines().any(|l| l.starts_with("KERNEL==\"uinput\"")));
    }

    #[test]
    fn the_manual_commands_carry_the_same_rule() {
        let rule = Rule { keyboard: false, uinput: true };
        let text = manual(Some(rule));
        assert!(text.contains(&rule_text(rule)) && text.contains("modprobe uinput"));
        assert!(manual(None).contains(&format!("rm -f {RULE_PATH}")));
    }
}
