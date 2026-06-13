//! `cargo xtask gen` — render the cheatsheet into the README table region and
//! the generated `docs/` artifacts (`index.html`, `sitemap.xml`, `llms.txt`).

use crate::cheatsheet::{
    self, Cheatsheet, Group, NonMapping, Row, README_END, README_START, SITE_URL,
};

pub fn run() -> Result<(), String> {
    let sheet = cheatsheet::load()?;

    let readme_path = cheatsheet::readme_path();
    let readme = std::fs::read_to_string(&readme_path)
        .map_err(|e| format!("reading {}: {e}", readme_path.display()))?;
    let updated = splice_readme(&readme, &render_readme_table(&sheet))?;
    write_if_changed(&readme_path, &updated)?;

    // `index.html` and `sitemap.xml` share one `today()` so the JSON-LD
    // `dateModified` and the sitemap `lastmod` never drift apart.
    let date = today();
    write_if_changed(
        &cheatsheet::html_output_path(),
        &render_html(&sheet, &date)?,
    )?;
    write_if_changed(&cheatsheet::sitemap_output_path(), &render_sitemap(&date))?;
    write_if_changed(&cheatsheet::llms_output_path(), &render_llms(&sheet)?)?;
    Ok(())
}

fn write_if_changed(path: &std::path::Path, contents: &str) -> Result<(), String> {
    let current = std::fs::read_to_string(path).unwrap_or_default();
    if current != contents {
        std::fs::write(path, contents).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Replace the text between the README markers (markers kept) with `table`.
pub fn splice_readme(readme: &str, table: &str) -> Result<String, String> {
    let start = readme
        .find(README_START)
        .ok_or_else(|| format!("README is missing the `{README_START}` marker"))?;
    let end = readme
        .find(README_END)
        .ok_or_else(|| format!("README is missing the `{README_END}` marker"))?;
    if end < start {
        return Err("README cheatsheet markers are out of order".to_owned());
    }
    let mut out = String::with_capacity(readme.len());
    out.push_str(&readme[..start]);
    out.push_str(README_START);
    out.push('\n');
    out.push_str(table);
    out.push_str(README_END);
    out.push_str(&readme[end + README_END.len()..]);
    Ok(out)
}

/// The README table is the compact view: one table per group, plus a short
/// pointer to the full GitHub Pages cheatsheet. The full "When NOT to use
/// slice" and caveats sections live on Pages to keep the README lean.
pub fn render_readme_table(sheet: &Cheatsheet) -> String {
    let mut out = String::new();
    out.push_str(
        "The recipes below are generated from `docs/cheatsheet.toml`; the full\n\
         version (byte ranges, every-Nth-line, NUL records, caveats, and a\n\
         \"when NOT to use slice\" section) lives at\n\
         <https://chantsune.github.io/slice/>.\n\n",
    );
    for group in Group::ORDER {
        let rows: Vec<&Row> = sheet.rows.iter().filter(|r| r.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str("#### ");
        out.push_str(group.title());
        out.push_str("\n\n");
        out.push_str("| Task | coreutils / sed / awk / dd | slice |\n");
        out.push_str("| --- | --- | --- |\n");
        for row in rows {
            out.push_str(&format!(
                "| {} | `{}` | `{}` |\n",
                md_escape(&row.task),
                md_escape(&row.tools),
                md_escape(&row.slice),
            ));
        }
        out.push('\n');
    }
    out
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

fn render_html(sheet: &Cheatsheet, date: &str) -> Result<String, String> {
    let template_path = cheatsheet::template_path();
    let template = std::fs::read_to_string(&template_path)
        .map_err(|e| format!("reading {}: {e}", template_path.display()))?;

    let body = render_html_body(sheet);
    let caveats = render_html_caveats(sheet);

    let html = template
        .replace("__CHEATSHEET_BODY__", &body)
        .replace("__CAVEATS_BODY__", &caveats)
        .replace("__DATE_MODIFIED__", date);

    // Guard against a renamed or typo'd placeholder shipping unsubstituted.
    for placeholder in [
        "__CHEATSHEET_BODY__",
        "__CAVEATS_BODY__",
        "__DATE_MODIFIED__",
    ] {
        if html.contains(placeholder) {
            return Err(format!(
                "template placeholder {placeholder} was not substituted"
            ));
        }
    }
    Ok(html)
}

fn render_html_body(sheet: &Cheatsheet) -> String {
    let mut out = String::new();
    for group in Group::ORDER {
        let rows: Vec<&Row> = sheet.rows.iter().filter(|r| r.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "<h2 id=\"{}\">{}</h2>\n",
            group.anchor(),
            html_escape(group.title())
        ));
        out.push_str("<table>\n<thead>\n<tr><th scope=\"col\">Task</th><th scope=\"col\">head · tail / sed · awk / dd</th><th scope=\"col\">slice</th></tr>\n</thead>\n<tbody>\n");
        for row in rows {
            out.push_str(&format!(
                "<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td></tr>\n",
                html_escape(&row.task),
                html_escape(&row.tools),
                html_escape(&row.slice),
            ));
            if let Some(note) = &row.note {
                out.push_str(&format!(
                    "<tr class=\"note\"><td colspan=\"3\">{}</td></tr>\n",
                    html_escape(note)
                ));
            }
        }
        out.push_str("</tbody>\n</table>\n");
    }
    out.push_str(&render_html_non_mappings(&sheet.non_mappings));
    out
}

fn render_html_non_mappings(non_mappings: &[NonMapping]) -> String {
    if non_mappings.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("<h2 id=\"when-not-to-use-slice\">When NOT to use slice</h2>\n");
    out.push_str(
        "<p>slice selects positional ranges and copies them through unchanged. \
         These jobs need a different tool:</p>\n",
    );
    out.push_str("<table>\n<thead>\n<tr><th scope=\"col\">Job</th><th scope=\"col\">Use this instead</th><th scope=\"col\">Why not slice</th></tr>\n</thead>\n<tbody>\n");
    for nm in non_mappings {
        out.push_str(&format!(
            "<tr><td>{}</td><td><code>{}</code></td><td>{}</td></tr>\n",
            html_escape(&nm.label),
            html_escape(&nm.cmd),
            html_escape(&nm.why_not),
        ));
    }
    out.push_str("</tbody>\n</table>\n");
    out
}

fn render_html_caveats(sheet: &Cheatsheet) -> String {
    if sheet.caveats.items.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("<h2 id=\"caveats\">Caveats</h2>\n<ul>\n");
    for item in &sheet.caveats.items {
        out.push_str(&format!("<li>{}</li>\n", html_escape(item)));
    }
    out.push_str("</ul>\n");
    out
}

/// The sitemap is a single-URL document; its `lastmod` is the same `today()`
/// the JSON-LD `dateModified` uses, so the two cannot drift.
fn render_sitemap(date: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
         \x20 <url>\n\
         \x20   <loc>{SITE_URL}</loc>\n\
         \x20   <lastmod>{date}</lastmod>\n\
         \x20   <changefreq>monthly</changefreq>\n\
         \x20   <priority>1.0</priority>\n\
         \x20 </url>\n\
         </urlset>\n"
    )
}

/// Render llms.txt from its template, substituting the SSOT-derived cheatsheet
/// body and the "when NOT to use" list so the file never drifts from the data.
fn render_llms(sheet: &Cheatsheet) -> Result<String, String> {
    let template_path = cheatsheet::llms_template_path();
    let template = std::fs::read_to_string(&template_path)
        .map_err(|e| format!("reading {}: {e}", template_path.display()))?;

    // Trim trailing newlines so the template's own blank lines around each
    // placeholder are the single source of vertical spacing.
    let body = render_llms_body(sheet);
    let non_mappings = render_llms_non_mappings(&sheet.non_mappings);
    let out = template
        .replace("__CHEATSHEET_BODY__", body.trim_end())
        .replace("__NON_MAPPINGS_BODY__", non_mappings.trim_end());

    for placeholder in ["__CHEATSHEET_BODY__", "__NON_MAPPINGS_BODY__"] {
        if out.contains(placeholder) {
            return Err(format!(
                "llms.txt template placeholder {placeholder} was not substituted"
            ));
        }
    }
    Ok(out)
}

fn render_llms_body(sheet: &Cheatsheet) -> String {
    let mut out = String::new();
    for group in Group::ORDER {
        let rows: Vec<&Row> = sheet.rows.iter().filter(|r| r.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(group.llms_label());
        out.push('\n');
        for row in rows {
            // A row with no coreutils equivalent (`tools = "—"`) lists only the
            // slice command plus its note; everything else maps tools -> slice.
            if row.tools == "—" {
                out.push_str(&format!("- {}: `{}`", row.task, row.slice));
                if let Some(note) = &row.note {
                    out.push_str(&format!(" ({})", strip_backticks(note)));
                }
                out.push('\n');
            } else {
                let tools = format_llms_tools(&row.tools, row.verify == cheatsheet::Verify::Gnu);
                out.push_str(&format!("- {}: {} -> `{}`\n", row.task, tools, row.slice));
            }
        }
        out.push('\n');
    }
    out
}

/// Wrap each `  /  `-separated tool spelling in backticks, appending `(GNU)`
/// when the row is GNU-only.
fn format_llms_tools(tools: &str, gnu: bool) -> String {
    let spellings: Vec<String> = tools
        .split("  /  ")
        .map(str::trim)
        .map(|s| format!("`{s}`"))
        .collect();
    let joined = spellings.join(" / ");
    if gnu {
        format!("{joined} (GNU)")
    } else {
        joined
    }
}

fn render_llms_non_mappings(non_mappings: &[NonMapping]) -> String {
    let mut out = String::new();
    for nm in non_mappings {
        out.push_str(&format!(
            "- {}: `{}` ({})\n",
            nm.label,
            nm.cmd,
            strip_trailing_period(&nm.why_not)
        ));
    }
    out
}

/// Drop a single trailing period so the parenthetical reads cleanly inline.
fn strip_trailing_period(s: &str) -> &str {
    s.strip_suffix('.').unwrap_or(s)
}

/// Remove backtick code spans from prose folded into a plain-text parenthetical.
fn strip_backticks(s: &str) -> String {
    s.replace('`', "")
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `dateModified` for the JSON-LD. Derived from `SOURCE_DATE_EPOCH` when set
/// (reproducible builds) so the generated HTML stays byte-stable in CI; falls
/// back to a fixed date otherwise rather than embedding the wall clock, which
/// would make `gen` non-idempotent.
fn today() -> String {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(secs) = epoch.parse::<i64>() {
            return date_from_epoch(secs);
        }
    }
    // Last meaningful content change; bump when the cheatsheet data changes.
    "2026-06-13".to_owned()
}

/// Civil date (UTC) from a Unix timestamp, via Howard Hinnant's algorithm.
fn date_from_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
