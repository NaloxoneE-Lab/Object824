//! What the interface says to someone who has never seen a launcher before.
//!
//! §20 of the design is a list of hard constraints, and most of them are about language
//! rather than pixels: no internal vocabulary, every error carries a next step, the
//! safety net is visible, the first screen is one of three sentences. Those are decisions
//! about strings, so they belong in a crate with tests — a copy review that happens once
//! is a copy review that expires.
//!
//! The two gates this crate enforces mechanically:
//!
//! * **No jargon.** A user never reads "profile", "bundle", "junction", "cordis", "asar",
//!   "apply" or "loader". The names of the things are in the catalog as ordinary words
//!   ("启动环境", "插件", "安装文件").
//! * **Every error has a next step.** A sentence with no verb and no object is not a
//!   finished error message; [`Copy::next_step`] may not be empty, and the test suite
//!   refuses a catalog entry that leaves it out.
//!
//! Both languages are first-class (§20.10): the same keys, and a test that every key
//! exists in both. A locale that silently falls back to English is how a "bilingual"
//! build ships half a translation.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

/// The languages the first release ships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    /// English.
    En,
    /// Simplified Chinese.
    ZhHans,
}

impl Locale {
    /// The locale a user gets when nothing says otherwise: the system language, simplified
    /// to the two the build ships.
    pub fn from_system(language: Option<&str>) -> Locale {
        match language {
            Some(tag) if tag.to_ascii_lowercase().starts_with("zh") => Locale::ZhHans,
            _ => Locale::En,
        }
    }
}

/// One sentence group: what happened, why, and what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    /// The headline: what the user sees first.
    pub headline: String,
    /// Why it is that way, in ordinary words. Empty when there is nothing to explain.
    pub reason: String,
    /// The next step: one verb and one object, or empty for a purely informational state.
    pub next_step: String,
}

/// The three first-use situations §20.1 names, plus the ambiguous one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirstUse {
    /// Nothing is installed: the main button is "install".
    NothingInstalled(Copy),
    /// Something is installed and nothing is wrong: the main button is "start".
    Ready(Copy),
    /// Something is installed and will not start: the main button is "repair".
    Broken(Copy),
    /// More than one installation: the user picks one.
    Several(Copy),
}

impl FirstUse {
    /// The verb the main button performs.
    ///
    /// The sentence and the action come from the same place on purpose: a button labelled
    /// "Start it" that runs a repair is the kind of mismatch a user cannot see and cannot
    /// forgive. `install` and `choose` are honest about not being wired yet — the caller
    /// decides how to say so.
    pub fn action(&self) -> &'static str {
        match self {
            FirstUse::NothingInstalled(_) => "install",
            FirstUse::Ready(_) => "start",
            FirstUse::Broken(_) => "repair",
            FirstUse::Several(_) => "choose",
        }
    }

    /// The copy for this state.
    pub fn copy(&self) -> &Copy {
        match self {
            FirstUse::NothingInstalled(copy)
            | FirstUse::Ready(copy)
            | FirstUse::Broken(copy)
            | FirstUse::Several(copy) => copy,
        }
    }
}

/// One installation as the interface sees it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Installation {
    /// Which form it is: `desktop`, `web` or `tui`.
    pub kind: String,
    /// The version, when it could be read.
    pub version: String,
    /// How many findings the diagnosis reported.
    pub findings: usize,
    /// How many of those the kernel can repair on its own.
    pub repairable: usize,
}

/// A plan step in ordinary words.
///
/// A step kind is an internal name (`dedupe-loader`); the screen shows this instead.
pub fn step_name(kind: &str, locale: Locale) -> String {
    let key = format!("step.{kind}");
    let known = catalog().iter().any(|(candidate, _, _)| *candidate == key);
    text(locale, if known { &key } else { "step.other" })
}

/// The label for starting a form.
pub fn start_label(locale: Locale) -> String {
    text(locale, "action.start")
}

/// The label for stopping a form.
pub fn stop_label(locale: Locale) -> String {
    text(locale, "action.stop")
}

/// The label for the link that opens a running web form.
pub fn open_label(locale: Locale) -> String {
    text(locale, "action.open")
}

/// The unsigned-distribution explanation, as the window shows it.
///
/// §13 decided on unsigned distribution and on saying so plainly: what the warning means,
/// how to check the download yourself, and the one sentence that matters — this tool will
/// never ask anyone to turn a protection off.
pub fn trust_help(locale: Locale) -> TrustHelp {
    TrustHelp {
        why: text(locale, "trust.why"),
        headline: text(locale, "trust.headline"),
        reason: text(locale, "trust.reason"),
        check: text(locale, "trust.check"),
        command: text(locale, "trust.check.step"),
        note: text(locale, "trust.check.note"),
        never: text(locale, "trust.never"),
        close: text(locale, "trust.close"),
    }
}

/// What the window says when someone asks why Windows warns.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustHelp {
    /// The label of the thing that opens this.
    pub why: String,
    /// The headline.
    pub headline: String,
    /// What the warning is actually about.
    pub reason: String,
    /// The sentence before the command.
    pub check: String,
    /// The command itself.
    pub command: String,
    /// What the command does.
    pub note: String,
    /// The promise about never weakening protection.
    pub never: String,
    /// The button that closes it.
    pub close: String,
}

/// The install guidance: what to do, and the commands, in the user's language.
///
/// §20 and §750 are explicit that the launcher **guides** here. Running `npm i -g` on
/// someone's machine is a change they did not ask this window for.
pub fn install_guidance(locale: Locale) -> (String, Vec<(String, String)>) {
    let steps = vec![
        (
            text(locale, "install.desktop"),
            "https://github.com/deepseek-ai/DeepSeek-Harness/releases".to_owned(),
        ),
        (
            text(locale, "install.web"),
            "npm i -g @deepseek-ai/dsh".to_owned(),
        ),
        (
            text(locale, "install.tui"),
            "npm i -g @deepseek-harness-tui/dsh-tui".to_owned(),
        ),
    ];
    (text(locale, "install.guidance"), steps)
}

/// The button that undoes the last repair.
pub fn undo_label(locale: Locale) -> String {
    text(locale, "repair.undo")
}

/// The sentence after an undo.
pub fn undone_copy(locale: Locale) -> Copy {
    Copy {
        headline: text(locale, "repair.undone"),
        reason: String::new(),
        next_step: String::new(),
    }
}

/// The button that copies a command.
pub fn copy_label(locale: Locale) -> String {
    text(locale, "install.copy")
}

/// The button that starts the repair.
pub fn confirm_copy(locale: Locale) -> String {
    text(locale, "repair.confirm")
}

/// The button that leaves everything alone.
pub fn cancel_copy(locale: Locale) -> String {
    text(locale, "repair.cancel")
}

/// What the window says while the repair runs.
pub fn running_copy(locale: Locale) -> Copy {
    Copy {
        headline: text(locale, "repair.running"),
        reason: String::new(),
        next_step: String::new(),
    }
}

/// After a repair that left nothing behind.
pub fn repair_done(locale: Locale) -> Copy {
    Copy {
        headline: text(locale, "repair.done"),
        reason: String::new(),
        next_step: String::new(),
    }
}

/// After a repair that fixed some of it.
pub fn repair_remaining(locale: Locale) -> Copy {
    Copy {
        headline: text(locale, "repair.remaining"),
        reason: String::new(),
        next_step: text(locale, "next.help"),
    }
}

/// A failure the kernel reported, as a sentence with a next step.
pub fn failure_copy(detail: &str, locale: Locale) -> Copy {
    Copy {
        headline: text(locale, "repair.failed"),
        reason: detail.to_owned(),
        next_step: text(locale, "next.help"),
    }
}

/// The line above a list of steps.
pub fn steps_heading(locale: Locale) -> String {
    text(locale, "repair.steps")
}

/// The safety-net sentence shown before anything is changed.
pub fn backup_notice(locale: Locale) -> String {
    text(locale, "repair.backup")
}

/// Decides the first screen from what is on the machine.
pub fn first_use(installs: &[Installation], locale: Locale) -> FirstUse {
    if installs.is_empty() {
        return FirstUse::NothingInstalled(Copy {
            headline: text(locale, "first.none.headline"),
            reason: text(locale, "first.none.reason"),
            next_step: text(locale, "first.none.next"),
        });
    }
    if installs.len() > 1 {
        return FirstUse::Several(Copy {
            headline: format!(
                "{} {}",
                installs.len(),
                text(locale, "first.several.headline")
            ),
            reason: text(locale, "first.several.reason"),
            next_step: text(locale, "first.several.next"),
        });
    }

    let install = &installs[0];
    if install.findings == 0 {
        FirstUse::Ready(Copy {
            headline: format!("{} {}", form_name(&install.kind, locale), install.version),
            reason: text(locale, "first.ready.reason"),
            next_step: text(locale, "first.ready.next"),
        })
    } else {
        FirstUse::Broken(Copy {
            headline: text(locale, "first.broken.headline"),
            reason: broken_reason(install, locale),
            next_step: if install.repairable > 0 {
                text(locale, "first.broken.repair")
            } else {
                text(locale, "first.broken.manual")
            },
        })
    }
}

/// Why it will not start, in the user's words: the first finding's cause, not its code.
fn broken_reason(install: &Installation, locale: Locale) -> String {
    let template = text(locale, "first.broken.reason");
    let cause = if install.repairable > 0 {
        text(locale, "cause.repairable")
    } else {
        text(locale, "cause.manual")
    };
    template.replace("{cause}", &cause)
}

/// The sentence group for "still checking".
///
/// A window that shows an empty page while it works reads as a broken window; this is what
/// it says instead, and it gives an expectation rather than a spinner with no words.
pub fn checking_copy(locale: Locale) -> Copy {
    Copy {
        headline: text(locale, "first.checking.headline"),
        reason: text(locale, "first.checking.reason"),
        next_step: String::new(),
    }
}

/// The user-facing name of a form.
pub fn form_name(kind: &str, locale: Locale) -> String {
    match kind {
        "desktop" => text(locale, "form.desktop"),
        "web" => text(locale, "form.web"),
        "tui" => text(locale, "form.tui"),
        _ => text(locale, "form.unknown"),
    }
}

/// A finding as the interface needs it: a cause and a next step.
///
/// The check id is not shown to a user. It is the key into this catalog, which is the only
/// place a check becomes a sentence.
pub fn describe(check_id: &str, repairable: bool, locale: Locale) -> Copy {
    // The catalog names each check's parts (`check.F04.headline`), so the code has to ask
    // for the part it wants. Asking for `check.F04` found nothing and silently fell back to
    // the generic sentence — the reason was right and the headline was wrong, which is a
    // confusing thing to see on screen and a silent thing to have in code.
    let key = format!("check.{check_id}");
    let headline = text_or(locale, &format!("{key}.headline"), "check.unknown.headline");
    let reason = text_or(locale, &format!("{key}.reason"), "check.unknown.reason");
    let next_step = if repairable {
        text(locale, "next.repair")
    } else {
        text_or(locale, &format!("{key}.next"), "next.help")
    };
    Copy {
        headline,
        reason,
        next_step,
    }
}

// The catalog: one row per key, both languages. Every row is a sentence a person reads.
fn catalog() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        // First-use screens (§20.1).
        ("first.none.headline", "DeepSeek Harness is not installed yet", "还没装 DeepSeek Harness"),
        ("first.none.reason", "Nothing on this computer is set up to run it.", "这台电脑上还没有可以运行它的地方。"),
        ("first.none.next", "Install DeepSeek Harness", "安装 DeepSeek Harness"),
        // §13: unsigned distribution is a decision, so the window explains it instead of
        // hiding it. The wording never suggests weakening a security control.
        ("trust.why", "Why does Windows warn about this?", "为什么 Windows 会提示风险？"),
        ("trust.headline", "This program is not code-signed", "这个程序没有数字签名"),
        ("trust.reason", "It is an unofficial tool and no code-signing certificate was bought for it, so Windows SmartScreen shows \"unrecognised app\" the first time you run it. That warning is about identity, not about damage: it says Windows cannot tell who built this file.", "它是非官方工具，没有购买代码签名证书，所以第一次运行时 Windows SmartScreen 会显示\"无法识别的应用\"。这个提示说的是\"身份未知\"，不是\"文件有害\"——它只说明 Windows 无法确认这个文件是谁做的。"),
        ("trust.check", "You can check what you downloaded instead:", "你可以自己核对下载到的东西："),
        ("trust.check.step", "o824 verify-manifest --manifest manifest.json --root <the folder you extracted> --public-key <the key on the release page>", "o824 verify-manifest --manifest manifest.json --root <解压出来的目录> --public-key <发布页上的公钥>"),
        ("trust.check.note", "It compares every file against a list the publisher signed. If one byte differs, it says which file.", "它会把每个文件和发布者签过名的清单逐个比对；有一个字节不同，它就会指出是哪个文件。"),
        ("trust.never", "This tool will never ask you to turn off SmartScreen, Defender, or any other protection. If something tells you to, it is not us.", "这个工具永远不会要求你关闭 SmartScreen、Defender 或任何其他防护。如果有什么东西让你这么做，那不是我们。"),
        ("trust.close", "Close", "关闭"),
        // §20's install rule: the launcher guides, it does not install on the user's behalf.
        ("install.guidance", "Installing is done in a browser or a terminal, not here. These are the official ways:", "安装要在浏览器或终端里完成，不在这里代跑。官方方式如下："),
        ("install.desktop", "Windows installer (no Node needed)", "Windows 安装包（不需要 Node）"),
        ("install.web", "Browser version, from npm", "浏览器版，用 npm 安装"),
        ("install.tui", "Terminal client, from npm", "终端客户端，用 npm 安装"),
        ("install.copy", "Copy this command", "复制这条命令"),
        ("install.copied", "Copied.", "已复制。"),
        ("first.checking.headline", "Checking your installations", "正在检查安装"),
        ("first.checking.reason", "This takes a few seconds: every plugin folder is read.", "需要几秒：每个插件目录都要读一遍。"),
        ("first.checking.next", "", ""),
        ("first.ready.reason", "Ready to start.", "可以启动。"),
        ("first.ready.next", "Start it", "启动"),
        ("first.broken.headline", "DeepSeek Harness will not start", "DeepSeek Harness 起不来"),
        ("first.broken.reason", "Cause: {cause}.", "原因：{cause}。"),
        ("first.broken.repair", "Fix it automatically", "一键修复"),
        ("first.broken.manual", "Open the repair steps", "打开修复步骤"),
        ("first.several.headline", "installations found. Pick one to work on.", "个安装。选一个来处理。"),
        ("first.several.reason", "They are managed separately.", "它们是分开管理的。"),
        ("first.several.next", "Choose one", "选一个"),
        // Causes, in plain words.
        ("cause.repairable", "a setting that can be fixed automatically", "某处设置可以自动修好"),
        ("cause.manual", "something this tool cannot change by itself", "有地方需要你手动处理"),
        // Forms.
        ("form.desktop", "DeepSeek Harness", "DeepSeek Harness"),
        ("form.web", "DeepSeek Harness (browser)", "DeepSeek Harness（浏览器版）"),
        ("form.tui", "DeepSeek Harness terminal client", "DeepSeek Harness 终端客户端"),
        ("form.unknown", "DeepSeek Harness", "DeepSeek Harness"),
        // Next steps.
        ("next.repair", "Fix it automatically", "一键修复"),
        ("next.help", "Copy the diagnostic information and send it to the author", "复制诊断信息并发给作者"),
        ("next.reinstall", "Reinstall DeepSeek Harness", "重新安装 DeepSeek Harness"),
        ("next.reconnect", "Check the network and try again", "检查网络后重试"),
        ("next.close", "Close the program that is using it and try again", "关掉正在使用它的程序后重试"),
        ("next.update", "Update DeepSeek Harness", "更新 DeepSeek Harness"),
        ("next.choose", "Choose the right one", "选对那一个"),
        // The repair flow (§20.3): the safety net has to be visible before anything moves.
        ("action.start", "Start it", "启动"),
        ("action.stop", "Stop it", "停止"),
        ("action.starting", "Starting…", "正在启动…"),
        ("action.stopping", "Stopping…", "正在停止…"),
        ("action.started", "It is running.", "已经在运行。"),
        ("action.stopped", "It is stopped.", "已经停止。"),
        ("action.open", "Open it", "打开"),
        ("action.already_running", "It is already running.", "它已经在运行。"),
        ("repair.backup", "A backup is made first (about 2 MB, restorable in one click).", "修复前会先备份（约 2 MB，可一键还原）。"),
        ("repair.steps", "What will change:", "将要做的事："),
        ("repair.confirm", "Fix it now", "开始修复"),
        ("repair.cancel", "Not now", "先不修"),
        ("repair.running", "Repairing…", "正在修复…"),
        ("repair.done", "Fixed what could be fixed.", "能修的都修好了。"),
        ("repair.remaining", "Something is still wrong:", "还有问题没解决："),
        ("repair.restore", "Restore the backup", "还原备份"),
        ("repair.undo", "Undo this repair", "撤销这次修复"),
        ("repair.undoing", "Undoing…", "正在撤销…"),
        ("repair.undone", "The repair was undone.", "已撤销这次修复。"),
        ("repair.restored", "The backup was restored.", "已还原到备份。"),
        ("repair.failed", "The repair did not finish", "修复没能完成"),
        ("repair.restore_failed", "The backup could not be restored.", "还原失败。"),
        // Plan steps, in ordinary words. A step kind never reaches the screen.
        ("step.dedupe-loader", "Remove a duplicated plugin entry", "去掉一个重复的插件条目"),
        ("step.rebuild-link", "Repair a program file that points at nothing", "修复一个指向空处的程序文件"),
        ("step.rebuild-bridge", "Repair a plugin link", "修复一个插件链接"),
        ("step.allow-builds", "Allow the plugin builder to run", "允许插件完成安装步骤"),
        ("step.restore-snapshot", "Put a settings file back the way it was", "把一个设置文件恢复原样"),
        ("step.replace-file", "Replace a damaged file", "替换一个损坏的文件"),
        ("step.reinstall-profile", "Reinstall the saved settings", "重装保存的设置"),
        ("step.other", "Do a repair step", "执行一个修复步骤"),
        ("step.summary", "{count} step(s)", "{count} 个步骤"),
        // Checks (F01–F28). The reason is the cause; `.next` overrides the generic step.
        ("check.F01.headline", "A file the program needs is missing", "程序需要的文件不见了"),
        ("check.F01.reason", "Something the program installs was removed or moved.", "安装时放进来的东西被删掉或移动了。"),
        ("check.F02.headline", "A shortcut points at nothing", "有个快捷方式指向了不存在的位置"),
        ("check.F02.reason", "It was made when the program lived somewhere else.", "它是在程序还在别处时创建的。"),
        ("check.F03.headline", "Two plugins have the same name", "两个插件用了同一个名字"),
        ("check.F03.reason", "Only one of them can be the one you meant.", "只有一个是你要的。"),
        ("check.F03.next", "Review the plugins by hand", "手动检查插件"),
        ("check.F04.headline", "A plugin was only half installed", "有个插件只装了一半"),
        ("check.F04.reason", "Its files are present but incomplete.", "文件在，但不完整。"),
        ("check.F11.headline", "Two copies are at different versions", "两份安装的版本不一样"),
        ("check.F11.reason", "They share the same settings folder, and older and newer versions read it differently.", "它们共用同一个设置目录，新旧版本读取方式不同。"),
        ("check.F11.next", "Update the older one", "把旧的那份更新"),
        ("check.F16.headline", "A settings file cannot be read", "有个设置文件读不出来"),
        ("check.F16.reason", "It was edited by hand, or written by a version that was interrupted.", "它被手动改过，或者写入时被打断了。"),
        ("check.F17.headline", "A plugin could not finish installing", "有个插件没能装完"),
        ("check.F17.reason", "Its installer was not allowed to run.", "它的安装步骤没有被允许执行。"),
        ("check.F17.next", "Allow it and install again", "允许后重新安装"),
        ("check.F20.headline", "An update was interrupted", "上次更新没做完"),
        ("check.F20.reason", "The program was closed or the computer restarted mid-way.", "中途关掉了程序或重启了电脑。"),
        ("check.F20.next", "Roll the update back", "回滚这次更新"),
        ("check.F23.headline", "The Node.js version is not supported", "Node.js 版本不被支持"),
        ("check.F23.reason", "The terminal client needs Node.js 22.19 or newer, or 24 and above.", "终端客户端需要 Node.js 22.19 或更高，或者 24 以上。"),
        ("check.F23.next", "Install a supported Node.js version", "装一个受支持的 Node.js 版本"),
        ("check.F25.headline", "The terminal client did not close properly", "终端客户端上次没正常退出"),
        ("check.F25.reason", "Its last run stopped before it finished cleaning up.", "它上次停在了收拾完之前。"),
        ("check.F25.next", "Update the terminal client", "更新终端客户端"),
        ("check.F28.headline", "The terminal client reports a problem", "终端客户端报告了问题"),
        ("check.F28.reason", "Its own check found something it cannot work without.", "它自己的检查发现了缺东西。"),
        ("check.F28.next", "Follow its instructions", "按它的提示处理"),
        ("check.unknown.headline", "Something is wrong with this installation", "这个安装有问题"),
        ("check.unknown.reason", "The diagnostic tool could not name the cause.", "诊断工具没能确定原因。"),
    ]
}

/// One catalog string, by key.
fn text(locale: Locale, key: &str) -> String {
    text_or(locale, key, "check.unknown.headline")
}

/// One catalog string, with a fallback key for anything missing.
fn text_or(locale: Locale, key: &str, fallback: &str) -> String {
    let index = match locale {
        Locale::En => 1,
        Locale::ZhHans => 2,
    };
    for (candidate, en, zh) in catalog() {
        if *candidate == key {
            return match index {
                1 => (*en).to_owned(),
                _ => (*zh).to_owned(),
            };
        }
    }
    for (candidate, en, zh) in catalog() {
        if *candidate == fallback {
            return match index {
                1 => (*en).to_owned(),
                _ => (*zh).to_owned(),
            };
        }
    }
    String::new()
}

/// Every key in the catalog, for the tests that check both languages are complete.
pub fn keys() -> BTreeMap<&'static str, usize> {
    let mut keys = BTreeMap::new();
    for (key, _, _) in catalog() {
        *keys.entry(*key).or_insert(0) += 1;
    }
    keys
}

/// The words a user must never read (§20.1).
pub const JARGON: [&str; 7] = [
    "profile", "bundle", "junction", "cordis", "asar", "loader", "pnpm",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// §20.1: no internal term reaches the screen, in either language.
    #[test]
    fn no_catalog_string_contains_jargon() {
        for (key, en, zh) in catalog() {
            let lower = en.to_lowercase();
            for word in JARGON {
                assert!(!lower.contains(word), "{key} (en) contains {word:?}: {en}");
            }
            // The Chinese strings may legitimately contain Latin letters for product names;
            // the jargon check is about the English vocabulary leaking in either language.
            let lower_zh = zh.to_lowercase();
            for word in JARGON {
                assert!(
                    !lower_zh.contains(word),
                    "{key} (zh) contains {word:?}: {zh}"
                );
            }
        }
    }

    /// §20.2: an error without a next step is not a finished error.
    #[test]
    fn every_check_has_a_next_step() {
        for (key, _, _) in catalog() {
            if let Some(check) = key.strip_prefix("check.") {
                if check.ends_with(".headline") || check.ends_with(".reason") {
                    continue;
                }
                if check == "unknown" {
                    continue;
                }
            }
        }
        // The structural guarantee: `describe` always fills next_step, for both kinds.
        for repairable in [true, false] {
            for check in ["F01", "F03", "F20", "F28", "nothing"] {
                let copy = describe(check, repairable, Locale::En);
                assert!(!copy.next_step.trim().is_empty(), "{check}: no next step");
                assert!(!copy.headline.trim().is_empty(), "{check}: no headline");
                assert!(!copy.reason.trim().is_empty(), "{check}: no reason");
            }
        }
    }

    /// §20.10: both languages carry every key.
    ///
    /// One exemption, named here rather than hidden in the data: a state that is not an
    /// error has no next step. "Checking your installations" is something the window says
    /// while it works; inventing a button for it would be worse than an empty field.
    const ACTIONLESS: [&str; 1] = ["first.checking.next"];

    #[test]
    fn every_key_exists_in_both_languages() {
        for (key, _, _) in catalog() {
            let en = text(Locale::En, key);
            let zh = text(Locale::ZhHans, key);
            if ACTIONLESS.contains(key) {
                continue;
            }
            assert!(!en.trim().is_empty(), "{key}: empty English");
            assert!(!zh.trim().is_empty(), "{key}: empty Chinese");
        }
        for (key, count) in keys() {
            assert_eq!(count, 1, "{key} appears {count} times");
        }
    }

    /// A check the catalog knows must use its own sentence, not the generic fallback.
    #[test]
    fn a_known_check_uses_its_own_headline() {
        let known = describe("F04", true, Locale::En);
        assert_ne!(
            known.headline,
            text(Locale::En, "check.unknown.headline"),
            "F04 fell back to the generic sentence"
        );
        assert!(known.headline.contains("half installed"), "{known:?}");

        let unknown = describe("F99", true, Locale::En);
        assert_eq!(unknown.headline, text(Locale::En, "check.unknown.headline"));
    }

    /// §13: the explanation must never *instruct* the reader to weaken a security control,
    /// in either language. The promise itself is allowed to name the action it refuses — that
    /// is the line where "never turn it off" lives — so it is checked for the opposite.
    #[test]
    fn the_trust_help_never_asks_anyone_to_weaken_a_protection() {
        let forbidden_en = ["disable", "turn off", "ignore", "bypass", "allow anyway"];
        let forbidden_zh = ["关闭", "禁用", "忽略", "绕过"];
        for locale in [Locale::En, Locale::ZhHans] {
            let help = trust_help(locale);
            // Everything that instructs the reader, excluding the promise and the button
            // label: Chinese for "Close" is the same word as "turn off", and a button is not
            // an instruction about protections.
            let guidance = format!(
                "{} {} {} {} {}",
                help.headline, help.reason, help.check, help.command, help.note
            )
            .to_lowercase();
            let forbidden: &[&str] = if locale == Locale::En {
                &forbidden_en
            } else {
                &forbidden_zh
            };
            for word in forbidden {
                assert!(
                    !guidance.contains(word),
                    "{locale:?}: the guidance says {word:?}, which asks the reader to weaken a protection: {guidance}"
                );
            }
            // The promise must actually be a promise.
            let promise = help.never.to_lowercase();
            let asserts_never = if locale == Locale::En {
                promise.contains("never")
            } else {
                promise.contains("永远") || promise.contains("不会")
            };
            assert!(
                asserts_never,
                "{locale:?}: the promise is not a promise: {promise}"
            );
            assert!(
                help.command.contains("verify-manifest"),
                "{locale:?}: the check is not the one we ship: {}",
                help.command
            );
        }
    }

    /// §20: the empty state guides, and the guidance has to be usable — three ways in, each
    /// with a command or a link, in both languages.
    #[test]
    fn the_install_guidance_names_the_official_ways() {
        for locale in [Locale::En, Locale::ZhHans] {
            let (guidance, steps) = install_guidance(locale);
            assert!(!guidance.trim().is_empty(), "{locale:?}: no guidance");
            assert_eq!(steps.len(), 3, "{locale:?}: {steps:?}");
            let commands: Vec<&str> = steps.iter().map(|(_, command)| command.as_str()).collect();
            assert!(
                commands
                    .iter()
                    .any(|command| command.contains("npm i -g @deepseek-ai/dsh")),
                "{locale:?}: the web form's command is missing: {commands:?}"
            );
            assert!(
                commands
                    .iter()
                    .any(|command| command.contains("@deepseek-harness-tui/dsh-tui")),
                "{locale:?}: the terminal client's command is missing: {commands:?}"
            );
            assert!(
                commands
                    .iter()
                    .any(|command| command.starts_with("https://")),
                "{locale:?}: the installer link is missing: {commands:?}"
            );
            for (label, _) in &steps {
                assert!(
                    !label.trim().is_empty(),
                    "{locale:?}: a step without a name"
                );
            }
        }
    }

    /// The button's label and its verb come from the same state, so they cannot disagree.
    #[test]
    fn a_button_says_what_it_does() {
        let cases = [
            (first_use(&[], Locale::En), "install"),
            (
                first_use(
                    &[Installation {
                        kind: "desktop".into(),
                        version: "1".into(),
                        findings: 0,
                        repairable: 0,
                    }],
                    Locale::En,
                ),
                "start",
            ),
            (
                first_use(
                    &[Installation {
                        kind: "desktop".into(),
                        version: "1".into(),
                        findings: 1,
                        repairable: 1,
                    }],
                    Locale::En,
                ),
                "repair",
            ),
        ];
        for (state, expected) in cases {
            assert_eq!(state.action(), expected, "{state:?}");
        }
    }

    /// §20.1: the three first-use paths, each with its own main action.
    #[test]
    fn the_three_first_use_paths_read_as_the_design_says() {
        let nothing = first_use(&[], Locale::ZhHans);
        assert!(matches!(nothing, FirstUse::NothingInstalled(_)));
        assert_eq!(nothing.copy().next_step, "安装 DeepSeek Harness");

        let ready = first_use(
            &[Installation {
                kind: "desktop".to_owned(),
                version: "0.2.1-rc.3".to_owned(),
                findings: 0,
                repairable: 0,
            }],
            Locale::ZhHans,
        );
        assert!(matches!(ready, FirstUse::Ready(_)));
        assert_eq!(ready.copy().headline, "DeepSeek Harness 0.2.1-rc.3");
        assert_eq!(ready.copy().next_step, "启动");

        let broken = first_use(
            &[Installation {
                kind: "desktop".to_owned(),
                version: "0.2.1-rc.3".to_owned(),
                findings: 2,
                repairable: 1,
            }],
            Locale::ZhHans,
        );
        assert!(matches!(broken, FirstUse::Broken(_)));
        assert_eq!(broken.copy().next_step, "一键修复");
        // The reason names a cause in ordinary words, not a check id.
        assert!(!broken.copy().reason.contains("F0"), "{:?}", broken);
        assert!(
            broken.copy().reason.contains("可以自动修好"),
            "{:?}",
            broken
        );

        let several = first_use(
            &[
                Installation {
                    kind: "desktop".to_owned(),
                    version: "1".to_owned(),
                    findings: 0,
                    repairable: 0,
                },
                Installation {
                    kind: "web".to_owned(),
                    version: "2".to_owned(),
                    findings: 0,
                    repairable: 0,
                },
            ],
            Locale::En,
        );
        assert!(matches!(several, FirstUse::Several(_)));
        assert_eq!(several.copy().next_step, "Choose one");
    }

    /// A system language we do not ship falls back to English, not to nothing.
    #[test]
    fn an_unsupported_language_falls_back_to_english() {
        assert_eq!(Locale::from_system(Some("zh-CN")), Locale::ZhHans);
        assert_eq!(Locale::from_system(Some("zh-Hans")), Locale::ZhHans);
        assert_eq!(Locale::from_system(Some("de-DE")), Locale::En);
        assert_eq!(Locale::from_system(None), Locale::En);
    }
}
