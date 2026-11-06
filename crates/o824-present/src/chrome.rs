//! The words the window's own furniture says.
//!
//! The sentences that describe a machine (what is wrong, what to do about it) come from the
//! same catalog as before. These are the labels around them — the distribution rail, the
//! button over the start control, the settings dialog — and they live here for the same
//! reason: §20.10 says a hard-coded string in the interface is a bug, and a second language
//! that only covers half the screen is not a translation.
//!
//! Nothing in here names an internal concept. "Check the installation" is allowed;
//! "verify the asar bundle" is not, and the catalog test rejects it.

use crate::{text, Copy, Locale};

/// Every label the launcher shell shows, in one place.
///
/// Templates carry `{name}`-style placeholders rather than being assembled from fragments:
/// word order differs between the two languages, and a sentence built by concatenating
/// pieces is a sentence that reads like it was built by concatenating pieces.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Chrome {
    pub update: String,
    pub update_busy: String,
    pub update_ready: String,
    pub settings: String,
    pub minimize: String,
    pub close: String,
    pub distributions: String,
    pub all_installs: String,
    pub installed: String,
    pub not_installed: String,
    pub problems: String,
    pub launcher_settings: String,
    pub about: String,
    pub startup: String,
    pub autostart: String,
    pub autostart_detail: String,
    pub update_section: String,
    pub autoupdate: String,
    pub autoupdate_detail: String,
    pub close_section: String,
    pub tray: String,
    pub tray_detail: String,
    pub shortcut: String,
    pub shortcut_done: String,
    pub integrity: String,
    pub cache: String,
    pub cache_done: String,
    pub details: String,
    pub details_none: String,
    pub copy: String,
    pub copied: String,
    pub open_folder: String,
    pub open_log: String,
    pub tagline: String,
    pub version: String,
    pub repo: String,
    pub close_notice_title: String,
    pub close_notice_reason: String,
    pub close_keep: String,
    pub close_quit: String,
    pub close_remember: String,
    pub tray_unavailable: String,
    pub after_start: String,
    pub checking: String,
    pub setting_failed: String,
    pub update_title: String,
    pub update_checking: String,
    pub update_checking_detail: String,
    pub update_available: String,
    pub update_running: String,
    pub update_proof: String,
    pub update_install: String,
    pub update_later: String,
    pub update_failed: String,
    pub update_failed_detail: String,
    pub update_installing: String,
    pub update_installing_detail: String,
    pub update_done: String,
    pub update_broken: String,
    pub update_rolledback: String,
    pub update_none: String,
    pub update_none_detail: String,
    /// "More", the button that opens the four-item menu.
    pub more: String,
    /// The repair button's label, from the same sentence the first screen uses.
    pub repair: String,
    /// Reused from the action vocabulary rather than written twice.
    pub install: String,
    pub open: String,
    pub starting: String,
    pub stopping: String,
    pub started: String,
    pub stopped: String,
    pub already_running: String,
    /// The user-facing name of each distribution, for a rail slot that holds nothing yet.
    pub form_desktop: String,
    pub form_web: String,
    pub form_tui: String,
    /// Short names for the stage's eyebrow: the rail already says which product it is, so
    /// the line above the headline says which shape.
    pub kind_desktop: String,
    pub kind_web: String,
    pub kind_tui: String,
    /// Two technical failures that deserve a sentence instead of a kernel error string.
    pub nothing_to_repair: String,
    pub kernel_missing: String,
    pub kernel_hint: String,
    /// The header's top-up control.
    pub topup: String,
    /// The right rail: one label per link, plus the heading it belongs to.
    pub community: String,
    pub docs: String,
    pub mail: String,
    /// The announcement bar: two tabs, and the sentence for an empty feed.
    pub news: String,
    pub changelog: String,
    pub notice_empty: String,
}

/// The shell's labels for one language.
pub fn chrome(locale: Locale) -> Chrome {
    Chrome {
        update: text(locale, "chrome.update"),
        update_busy: text(locale, "chrome.update.busy"),
        update_ready: text(locale, "chrome.update.ready"),
        settings: text(locale, "chrome.settings"),
        minimize: text(locale, "chrome.minimize"),
        close: text(locale, "chrome.close"),
        distributions: text(locale, "chrome.distributions"),
        all_installs: text(locale, "chrome.all_installs"),
        installed: text(locale, "chrome.installed"),
        not_installed: text(locale, "chrome.not_installed"),
        problems: text(locale, "chrome.problems"),
        launcher_settings: text(locale, "chrome.launcher_settings"),
        about: text(locale, "chrome.about"),
        startup: text(locale, "chrome.startup"),
        autostart: text(locale, "chrome.autostart"),
        autostart_detail: text(locale, "chrome.autostart_detail"),
        update_section: text(locale, "chrome.update_section"),
        autoupdate: text(locale, "chrome.autoupdate"),
        autoupdate_detail: text(locale, "chrome.autoupdate_detail"),
        close_section: text(locale, "chrome.close_section"),
        tray: text(locale, "chrome.tray"),
        tray_detail: text(locale, "chrome.tray_detail"),
        shortcut: text(locale, "chrome.shortcut"),
        shortcut_done: text(locale, "chrome.shortcut_done"),
        integrity: text(locale, "chrome.integrity"),
        cache: text(locale, "chrome.cache"),
        cache_done: text(locale, "chrome.cache_done"),
        details: text(locale, "chrome.details"),
        details_none: text(locale, "chrome.details_none"),
        copy: text(locale, "chrome.copy"),
        copied: text(locale, "chrome.copied"),
        open_folder: text(locale, "chrome.open_folder"),
        open_log: text(locale, "chrome.open_log"),
        tagline: text(locale, "chrome.tagline"),
        version: text(locale, "chrome.version"),
        repo: text(locale, "chrome.repo"),
        close_notice_title: text(locale, "chrome.close_notice_title"),
        close_notice_reason: text(locale, "chrome.close_notice_reason"),
        close_keep: text(locale, "chrome.close_keep"),
        close_quit: text(locale, "chrome.close_quit"),
        close_remember: text(locale, "chrome.close_remember"),
        tray_unavailable: text(locale, "chrome.tray_unavailable"),
        after_start: text(locale, "chrome.after_start"),
        checking: text(locale, "chrome.checking"),
        setting_failed: text(locale, "chrome.setting_failed"),
        update_title: text(locale, "chrome.update_title"),
        update_checking: text(locale, "chrome.update_checking"),
        update_checking_detail: text(locale, "chrome.update_checking_detail"),
        update_available: text(locale, "chrome.update_available"),
        update_running: text(locale, "chrome.update_running"),
        update_proof: text(locale, "chrome.update_proof"),
        update_install: text(locale, "chrome.update_install"),
        update_later: text(locale, "chrome.update_later"),
        update_failed: text(locale, "chrome.update_failed"),
        update_failed_detail: text(locale, "chrome.update_failed_detail"),
        update_installing: text(locale, "chrome.update_installing"),
        update_installing_detail: text(locale, "chrome.update_installing_detail"),
        update_done: text(locale, "chrome.update_done"),
        update_broken: text(locale, "chrome.update_broken"),
        update_rolledback: text(locale, "chrome.update_rolledback"),
        update_none: text(locale, "chrome.update_none"),
        update_none_detail: text(locale, "chrome.update_none_detail"),
        more: text(locale, "chrome.more"),
        repair: text(locale, "first.broken.repair"),
        install: text(locale, "first.none.next"),
        open: text(locale, "action.open"),
        starting: text(locale, "action.starting"),
        stopping: text(locale, "action.stopping"),
        started: text(locale, "action.started"),
        stopped: text(locale, "action.stopped"),
        already_running: text(locale, "action.already_running"),
        form_desktop: crate::form_name("desktop", locale),
        form_web: crate::form_name("web", locale),
        form_tui: crate::form_name("tui", locale),
        kind_desktop: text(locale, "chrome.kind_desktop"),
        kind_web: text(locale, "chrome.kind_web"),
        kind_tui: text(locale, "chrome.kind_tui"),
        nothing_to_repair: text(locale, "chrome.nothing_to_repair"),
        kernel_missing: text(locale, "chrome.kernel_missing"),
        kernel_hint: text(locale, "chrome.kernel_hint"),
        topup: text(locale, "chrome.topup"),
        community: text(locale, "chrome.community"),
        docs: text(locale, "chrome.docs"),
        mail: text(locale, "chrome.mail"),
        news: text(locale, "chrome.news"),
        changelog: text(locale, "chrome.changelog"),
        notice_empty: text(locale, "chrome.notice_empty"),
    }
}

/// The first screen for a distribution this machine does not have.
///
/// The generic "DeepSeek Harness is not installed" is true and unhelpful when the other two
/// distributions *are* installed; the name is what tells the user which slot is empty.
pub fn absent_copy(kind: &str, locale: Locale) -> Copy {
    let name = crate::form_name(kind, locale);
    Copy {
        headline: text(locale, "first.absent.headline").replace("{name}", &name),
        reason: text(locale, "first.absent.reason").replace("{name}", &name),
        next_step: text(locale, "first.none.next"),
    }
}
