//! What the announcement bar shows: the DeepSeek API docs "新闻" and "更新日志" pages.
//!
//! The window cannot read these itself. The docs site answers without an
//! `Access-Control-Allow-Origin` header, so a `fetch()` from the WebView is refused before
//! it leaves the renderer; the pages are pulled here instead, parsed into a flat list, and
//! the bar draws that list and links out to the pages.
//!
//! Two properties matter for a launcher: the window must never wait on the network, and it
//! must still say something when the network is down. So the fetch runs off the UI thread,
//! the last good answer is cached on disk, and a failed refresh returns the cache marked
//! `stale` rather than an empty bar.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The news page. Its sidebar lists every release note, which is what the tab shows.
pub const NEWS_PAGE: &str = "https://api-docs.deepseek.com/zh-cn/news/news261017";
/// The changelog, in the human page's language.
pub const UPDATES_PAGE: &str = "https://api-docs.deepseek.com/zh-cn/updates";
/// The same changelog as Markdown: `## 时间: …` / `### …` pairs, with no HTML to scrape.
const UPDATES_SOURCE: &str = "https://api-docs.deepseek.com/zh-cn/updates.md";

/// How long a fetched answer is trusted before the next window open refreshes it.
const CACHE_TTL_SECONDS: u64 = 30 * 60;
/// How many rows each tab keeps. The bar is 146px tall; more than this is scrollback.
const LIMIT: usize = 12;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// `news` or `updates` — the bar draws one tab per source.
    pub source: String,
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Feed {
    pub items: Vec<Item>,
    pub fetched_at: u64,
    /// True when this is a cached answer we could not refresh.
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Feed {
    fn empty(error: Option<String>) -> Self {
        Self {
            items: Vec::new(),
            fetched_at: now(),
            stale: true,
            error,
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The bar's data: the cache when it is fresh, otherwise the pages, otherwise the cache.
pub fn feed(cache: Option<&Path>) -> Feed {
    let cached = cache.and_then(read_cache);
    if let Some(cached) = &cached {
        let age = now().saturating_sub(cached.fetched_at);
        if age < CACHE_TTL_SECONDS && !cached.items.is_empty() {
            return Feed {
                items: cached.items.clone(),
                fetched_at: cached.fetched_at,
                stale: false,
                error: None,
            };
        }
    }

    let mut items = Vec::new();
    let mut errors = Vec::new();
    match get(NEWS_PAGE) {
        Ok(html) => items.extend(parse_news(&html)),
        Err(error) => errors.push(format!("news: {error}")),
    }
    match get(UPDATES_SOURCE) {
        Ok(markdown) => items.extend(parse_updates(&markdown)),
        Err(error) => errors.push(format!("updates: {error}")),
    }

    let error = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };

    if items.is_empty() {
        // Nothing fresh and nothing cached: the bar has to be able to say "nothing yet".
        return match cached {
            Some(cached) => Feed {
                items: cached.items,
                fetched_at: cached.fetched_at,
                stale: true,
                error,
            },
            None => Feed::empty(error),
        };
    }

    let mut fresh = Feed {
        items,
        fetched_at: now(),
        stale: false,
        error,
    };
    if let Some(path) = cache {
        if let Err(failure) = write_cache(path, &fresh) {
            let note = format!("cache: {failure}");
            fresh.error = Some(match fresh.error.take() {
                Some(existing) => format!("{existing}; {note}"),
                None => note,
            });
        }
    }
    fresh
}

fn get(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(8))
        .user_agent("o824-gui/0.1 (+https://github.com/NaloxoneE-Lab/Object824)")
        .build();
    agent
        .get(url)
        .call()
        .map_err(|error| error.to_string())?
        .into_string()
        .map_err(|error| error.to_string())
}

fn read_cache(path: &Path) -> Option<Feed> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(path: &Path, feed: &Feed) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string(feed).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())
}

/// `## 时间: 2026-10-17` followed by one or more `### 标题`.
///
/// The Markdown is used rather than the rendered page because the headings are the whole
/// structure: no tags, no entities, no sidebar to tell apart from the body.
pub fn parse_updates(markdown: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut date = String::new();
    for line in markdown.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            date = heading_date(heading).unwrap_or_default();
        } else if let Some(heading) = line.strip_prefix("### ") {
            let title = clean_inline(heading);
            if title.is_empty() || date.is_empty() {
                continue;
            }
            items.push(Item {
                source: "updates".to_owned(),
                title,
                date: date.clone(),
                // Docusaurus builds the anchor from the heading text: `时间: 2026-10-17`
                // becomes `#时间-2026-10-17`. Browsers percent-encode it on the way out.
                url: format!("{UPDATES_PAGE}#时间-{date}"),
            });
            if items.len() >= LIMIT {
                break;
            }
        }
    }
    items
}

/// `时间: 2026-10-17` (or with a full-width colon) → `2026-10-17`.
fn heading_date(heading: &str) -> Option<String> {
    let (label, rest) = heading.split_once(':').or_else(|| heading.split_once('：'))?;
    if !label.trim().starts_with("时间") {
        return None;
    }
    let date = rest.trim();
    if is_date(date) {
        Some(date.to_owned())
    } else {
        None
    }
}

/// The news page's sidebar: one `<a class="menu__link" href="/zh-cn/news/news261017">` per
/// release note, with the date at the end of the link text.
///
/// Scanned rather than parsed: the bundle ships no HTML parser, and the two attributes this
/// needs are always in the same order. A duplicate href (the mobile and desktop sidebars are
/// both in the document) is kept once.
pub fn parse_news(html: &str) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("href=\"/zh-cn/news/news") {
        let after = &rest[start + "href=\"".len()..];
        let Some(quote) = after.find('"') else { break };
        let href = &after[..quote];
        let after_href = &after[quote..];
        let Some(gt) = after_href.find('>') else { break };
        let tail = &after_href[gt + 1..];
        let Some(close) = tail.find("</a>") else { break };
        rest = &tail[close..];

        if items.iter().any(|item| item.url.ends_with(href)) {
            continue;
        }
        let Some((title, date)) = split_title_date(&clean_inline(&tail[..close])) else {
            continue;
        };
        items.push(Item {
            source: "news".to_owned(),
            title,
            date,
            url: format!("https://api-docs.deepseek.com{href}"),
        });
        if items.len() >= LIMIT {
            break;
        }
    }
    items
}

/// `DeepSeek-V4.1-Flash-1017 发布 2026/10/17` → title and `2026-10-17`.
fn split_title_date(text: &str) -> Option<(String, String)> {
    let (title, tail) = text.rsplit_once(char::is_whitespace)?;
    let date = tail.trim().replace('/', "-");
    if !is_date(&date) {
        return None;
    }
    let title = title.trim().to_owned();
    if title.is_empty() {
        return None;
    }
    Some((title, date))
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// Drops tags and entities, collapses whitespace. The link text is the only place either
/// appears, and it is rendered by a static site generator, not by a person.
fn clean_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            // Docusaurus puts a zero-width space in every heading anchor.
            '\u{200b}' => {}
            _ => out.push(ch),
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_headings_become_items() {
        let markdown = "\
# 更新日志

***

## 时间: 2026-10-17

### 开放自研搜索 API

Body text.

### DeepSeek-V4.1-Flash-1017 更新

## 时间: 2026-09-10

### DeepSeek-V4.1-Flash 发布
";
        let items = parse_updates(markdown);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "开放自研搜索 API");
        assert_eq!(items[0].date, "2026-10-17");
        assert_eq!(items[0].source, "updates");
        assert_eq!(items[2].date, "2026-09-10");
        assert!(items[0].url.starts_with(UPDATES_PAGE));
    }

    #[test]
    fn a_heading_without_a_date_is_skipped() {
        let markdown = "### 没有日期的标题\n## 时间: 2026-10-17\n### 有日期\n";
        let items = parse_updates(markdown);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "有日期");
    }

    #[test]
    fn news_sidebar_links_become_items_once() {
        let html = r#"
            <a class="menu__link" href="/zh-cn/news/news261017">DeepSeek-V4.1-Flash-1017 发布 2026/10/17</a>
            <a class="menu__link" href="/zh-cn/news/news260910">DeepSeek-V4.1-Flash 发布 2026/09/10</a>
            <a class="menu__link" href="/zh-cn/news/news261017">DeepSeek-V4.1-Flash-1017 发布 2026/10/17</a>
            <a class="menu__link" href="/zh-cn/news/news250120">DeepSeek-R1 发布 2025/01/20</a>
        "#;
        let items = parse_news(html);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "DeepSeek-V4.1-Flash-1017 发布");
        assert_eq!(items[0].date, "2026-10-17");
        assert_eq!(
            items[0].url,
            "https://api-docs.deepseek.com/zh-cn/news/news261017"
        );
        assert_eq!(items[0].source, "news");
    }

    #[test]
    fn link_text_without_a_date_is_ignored() {
        let html = r#"<a class="menu__link" href="/zh-cn/news/news1">没有日期的标题</a>"#;
        assert!(parse_news(html).is_empty());
    }

    #[test]
    fn dates_are_shape_checked() {
        assert!(is_date("2026-10-17"));
        assert!(!is_date("2026/10/17"));
        assert!(!is_date("2026-1-7"));
        assert!(!is_date("时间: 2026"));
    }
}
