//! หน้าเว็บจัดการบอท: เลือกห้องแจ้งเตือนคนเข้า/ออก และห้องแสดงจำนวนสมาชิกจากห้องที่มีอยู่แล้ว
//! ป้องกันด้วยรหัสผ่าน (WEB_PASSWORD) และโดยค่าเริ่มต้นเปิดได้จากเครื่องนี้เท่านั้น

use std::fmt::Write as _;
use std::sync::Arc;

use axum::extract::{Form, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use poise::serenity_prelude as serenity;
use rand::distr::{Alphanumeric, SampleString};
use serde::Deserialize;

use crate::commands::truncate;
use crate::members::{self, Features};
use crate::music::MusicState;
use crate::settings::{DEFAULT_ONLINE_NAME, DEFAULT_TOTAL_NAME, LogKind, Settings};
use crate::stats;

/// ที่อยู่เริ่มต้น: เปิดได้จากเครื่องนี้เท่านั้น
const DEFAULT_ADDR: &str = "127.0.0.1:8080";

pub struct Web {
    ctx: serenity::Context,
    settings: Arc<Settings>,
    features: Features,
    music: Arc<MusicState>,
    password: String,
    /// กันเว็บอื่นแอบส่งฟอร์มมาแทนผู้ใช้ (CSRF)
    csrf: String,
}

/// เปิดเว็บในเบื้องหลัง ถ้าเปิดไม่ได้บอทยังทำงานต่อตามปกติ
pub fn spawn(
    ctx: serenity::Context,
    settings: Arc<Settings>,
    features: Features,
    music: Arc<MusicState>,
) {
    let addr = env_or("WEB_ADDR", DEFAULT_ADDR);
    if addr.eq_ignore_ascii_case("off") {
        return;
    }
    let (password, generated) = match std::env::var("WEB_PASSWORD") {
        Ok(p) if !p.trim().is_empty() => (p.trim().to_string(), false),
        _ => (Alphanumeric.sample_string(&mut rand::rng(), 12), true),
    };
    let web = Arc::new(Web {
        ctx,
        settings,
        features,
        music,
        password,
        csrf: Alphanumeric.sample_string(&mut rand::rng(), 32),
    });

    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[เว็บ] เปิดหน้าเว็บที่ {addr} ไม่ได้: {e} (ตั้งพอร์ตอื่นด้วย WEB_ADDR ใน .env)");
                return;
            }
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(8080);
        println!("[เว็บ] หน้าจัดการ: http://localhost:{port}");
        if generated {
            println!(
                "[เว็บ] รหัสผ่าน: {} (สุ่มใหม่ทุกครั้งที่เปิดบอท ตั้งเองได้ด้วย WEB_PASSWORD ใน .env)",
                web.password
            );
        }

        let app = axum::Router::new()
            .route("/", get(index))
            .route("/save", post(save))
            .route("/create-stats", post(create_stats))
            .layer(middleware::from_fn_with_state(web.clone(), guard))
            .with_state(web);
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("[เว็บ] หน้าเว็บหยุดทำงาน: {e}");
        }
    });
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// เทียบข้อความแบบใช้เวลาเท่ากันเสมอ กันการเดารหัสผ่านจากเวลาตอบ
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// ตรวจรหัสผ่าน (HTTP Basic) ทุกหน้า และห้ามเว็บอื่นฝังหน้านี้
async fn guard(State(web): State<Arc<Web>>, req: Request, next: Next) -> Response {
    let password = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| v.split_once(':').map(|(_, p)| p.to_string()));

    let mut res = if password.is_some_and(|p| same(&p, &web.password)) {
        next.run(req).await
    } else {
        let mut res = (
            StatusCode::UNAUTHORIZED,
            "ต้องใส่รหัสผ่าน (ดูได้จาก log ของบอท หรือ WEB_PASSWORD ใน .env)",
        )
            .into_response();
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"discord_bot\", charset=\"UTF-8\""),
        );
        res
    };
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[derive(Deserialize)]
struct GuildQuery {
    guild: Option<u64>,
}

#[derive(Deserialize)]
struct SaveForm {
    csrf: String,
    guild: u64,
    #[serde(default)]
    join_log: String,
    #[serde(default)]
    leave_log: String,
    #[serde(default)]
    stats_total: String,
    #[serde(default)]
    stats_online: String,
    #[serde(default)]
    total_name: String,
    #[serde(default)]
    online_name: String,
}

#[derive(Deserialize)]
struct ActionForm {
    csrf: String,
    guild: u64,
}

/// ผลลัพธ์ที่แสดงบนหน้า (สำเร็จ, ข้อความ)
type Notes = Vec<(bool, String)>;

async fn index(State(web): State<Arc<Web>>, Query(q): Query<GuildQuery>) -> Html<String> {
    Html(render(&web, q.guild, &[]))
}

/// ตรวจว่าเป็นฟอร์มจากหน้านี้จริง และเซิร์ฟเวอร์มีบอทอยู่
fn check_form(
    web: &Web,
    csrf: &str,
    guild: u64,
) -> Result<serenity::GuildId, (StatusCode, &'static str)> {
    if !same(csrf, &web.csrf) {
        return Err((
            StatusCode::FORBIDDEN,
            "ฟอร์มหมดอายุ (บอทอาจรีสตาร์ท) กลับไปโหลดหน้าใหม่แล้วลองอีกครั้ง",
        ));
    }
    let guild = serenity::GuildId::new(guild.max(1));
    if web.ctx.cache.guild(guild).is_none() {
        return Err((StatusCode::NOT_FOUND, "ไม่พบเซิร์ฟเวอร์นี้"));
    }
    Ok(guild)
}

/// "" = ไม่เลือก, ตัวเลข = ID ห้อง
fn parse_channel(value: &str) -> Option<serenity::ChannelId> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .map(serenity::ChannelId::new)
}

/// รูปแบบชื่อห้อง: ว่างหรือเท่าค่าเริ่มต้น = ใช้ค่าเริ่มต้น
fn clean_template(value: &str, default: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != default).then(|| truncate(value, stats::MAX_TEMPLATE))
}

async fn save(State(web): State<Arc<Web>>, Form(form): Form<SaveForm>) -> Response {
    let guild_id = match check_form(&web, &form.csrf, form.guild) {
        Ok(g) => g,
        Err(res) => return res.into_response(),
    };
    let mut notes: Notes = Vec::new();
    let mut push = |result: Result<String, crate::Error>| match result {
        Ok(text) => notes.push((true, text)),
        Err(e) => notes.push((false, e.to_string())),
    };
    let before = web.settings.get(guild_id);

    // รูปแบบชื่อห้อง
    let total_name = clean_template(&form.total_name, DEFAULT_TOTAL_NAME);
    let online_name = clean_template(&form.online_name, DEFAULT_ONLINE_NAME);
    let names_changed = total_name != before.total_name || online_name != before.online_name;
    if names_changed {
        let result = web
            .settings
            .update(guild_id, |s| {
                s.total_name = total_name;
                s.online_name = online_name;
            })
            .await;
        push(result.map(|()| "บันทึกรูปแบบชื่อห้องแล้ว".to_string()));
    }

    // ห้องแจ้งเตือนคนเข้า และคนออก (แยกห้องกัน หรือใช้ห้องเดียวกันก็ได้)
    for (kind, value) in [
        (LogKind::Join, &form.join_log),
        (LogKind::Leave, &form.leave_log),
    ] {
        let channel = parse_channel(value);
        if channel != before.log(kind) {
            let result = members::set_log(
                &web.ctx,
                &web.settings,
                guild_id,
                web.features,
                kind,
                channel,
            )
            .await;
            push(result);
        }
    }

    // ห้องสถิติ
    let total = parse_channel(&form.stats_total);
    let online = parse_channel(&form.stats_online);
    let ids_changed = (total.map(|c| c.get()), online.map(|c| c.get()))
        != (before.stats_total, before.stats_online);
    if ids_changed || (names_changed && before.has_stats()) {
        push(
            stats::use_existing(
                &web.ctx,
                &web.settings,
                guild_id,
                web.features,
                total,
                online,
            )
            .await,
        );
    }

    if notes.is_empty() {
        notes.push((true, "ไม่มีอะไรเปลี่ยน".to_string()));
    }
    Html(render(&web, Some(guild_id.get()), &notes)).into_response()
}

async fn create_stats(State(web): State<Arc<Web>>, Form(form): Form<ActionForm>) -> Response {
    let guild_id = match check_form(&web, &form.csrf, form.guild) {
        Ok(g) => g,
        Err(res) => return res.into_response(),
    };
    let note = match stats::create(&web.ctx, &web.settings, guild_id, web.features).await {
        Ok(text) => (true, text),
        Err(e) => (false, e.to_string()),
    };
    Html(render(&web, Some(guild_id.get()), &[note])).into_response()
}

/// แปลงอักขระพิเศษของ HTML
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
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

/// ข้อความใน Discord เช่น <#123> ให้แสดงเป็นชื่อห้องบนเว็บ
fn discord_to_text(text: &str, channels: &[(u64, String)]) -> String {
    let mut out = text.to_string();
    for (id, name) in channels {
        out = out.replace(&format!("<#{id}>"), &format!("#{name}"));
    }
    out
}

struct Channel {
    id: u64,
    label: String,
    kind: serenity::ChannelType,
}

/// ข้อมูลเซิร์ฟเวอร์ที่ใช้แสดงผล (คัดลอกออกจาก cache)
struct View {
    guild_id: u64,
    name: String,
    channels: Vec<Channel>,
    total: u64,
    online: Option<u64>,
}

fn snapshot(web: &Web, guild_id: serenity::GuildId) -> Option<View> {
    let guild = web.ctx.cache.guild(guild_id)?;
    let mut list: Vec<&serenity::GuildChannel> = guild.channels.values().collect();
    // เรียงตามหมวดหมู่แล้วตามตำแหน่ง เหมือนใน Discord
    let category_pos = |c: &serenity::GuildChannel| match c.kind {
        serenity::ChannelType::Category => (c.position, 0),
        _ => (
            c.parent_id
                .and_then(|p| guild.channels.get(&p))
                .map_or(0, |p| p.position),
            1,
        ),
    };
    list.sort_by_key(|c| (category_pos(c), c.position, c.id));

    let channels = list
        .into_iter()
        .map(|c| {
            let parent = c
                .parent_id
                .and_then(|p| guild.channels.get(&p))
                .map(|p| format!("{} / ", p.name))
                .unwrap_or_default();
            let kind = match c.kind {
                serenity::ChannelType::Category => "[หมวดหมู่] ",
                serenity::ChannelType::Voice => "[เสียง] ",
                serenity::ChannelType::Stage => "[เวที] ",
                _ => "#",
            };
            Channel {
                id: c.id.get(),
                label: format!("{parent}{kind}{}", c.name),
                kind: c.kind,
            }
        })
        .collect();
    let counts = members::count(&guild, web.features);
    Some(View {
        guild_id: guild_id.get(),
        name: guild.name.clone(),
        channels,
        total: counts.total,
        online: counts.online.map(|o| o.total()),
    })
}

fn options(channels: &[Channel], selected: Option<u64>, keep: impl Fn(&Channel) -> bool) -> String {
    let mut html = String::new();
    let none = if selected.is_none() { " selected" } else { "" };
    let _ = write!(html, "<option value=\"\"{none}>ไม่ใช้</option>");
    for c in channels.iter().filter(|c| keep(c)) {
        let sel = if selected == Some(c.id) {
            " selected"
        } else {
            ""
        };
        let _ = write!(
            html,
            "<option value=\"{}\"{sel}>{}</option>",
            c.id,
            esc(&c.label)
        );
    }
    html
}

fn render(web: &Web, wanted: Option<u64>, notes: &[(bool, String)]) -> String {
    let guilds: Vec<(u64, String)> = web
        .ctx
        .cache
        .guilds()
        .into_iter()
        .filter_map(|id| web.ctx.cache.guild(id).map(|g| (id.get(), g.name.clone())))
        .collect();
    let picked = wanted
        .filter(|w| guilds.iter().any(|(id, _)| id == w))
        .or_else(|| guilds.first().map(|(id, _)| *id));
    let bot_name = web.ctx.cache.current_user().name.clone();

    let Some(view) = picked.and_then(|id| snapshot(web, serenity::GuildId::new(id))) else {
        return page(
            &bot_name,
            "<p class=\"card\">บอทยังไม่ได้อยู่ในเซิร์ฟเวอร์ไหน หรือยังโหลดข้อมูลไม่เสร็จ ลองรีเฟรชอีกครั้ง</p>",
        );
    };
    let config = web.settings.get(serenity::GuildId::new(view.guild_id));
    let names: Vec<(u64, String)> = view
        .channels
        .iter()
        .map(|c| (c.id, c.label.clone()))
        .collect();

    let mut body = String::new();

    if guilds.len() > 1 {
        body.push_str("<nav class=\"guilds\">");
        for (id, name) in &guilds {
            let class = if *id == view.guild_id {
                " class=\"active\""
            } else {
                ""
            };
            let _ = write!(body, "<a href=\"/?guild={id}\"{class}>{}</a>", esc(name));
        }
        body.push_str("</nav>");
    }

    for (ok, text) in notes {
        let class = if *ok { "ok" } else { "err" };
        let text = esc(&discord_to_text(text, &names)).replace('\n', "<br>");
        let _ = write!(body, "<p class=\"note {class}\">{text}</p>");
    }

    let online = view
        .online
        .map_or_else(|| "ต้องเปิด PRESENCE INTENT".to_string(), |n| n.to_string());
    let music = if web.music.ytdlp_ready() {
        "พร้อม"
    } else {
        "ไม่พบ yt-dlp"
    };
    let _ = write!(
        body,
        "<section class=\"stats\">\
           <div><span>เซิร์ฟเวอร์</span><b>{}</b></div>\
           <div><span>สมาชิกทั้งหมด</span><b>{}</b></div>\
           <div><span>ออนไลน์</span><b>{}</b></div>\
           <div><span>ระบบเพลง</span><b>{music}</b></div>\
         </section>",
        esc(&view.name),
        view.total,
        esc(&online),
    );

    let is_text = |c: &Channel| {
        matches!(
            c.kind,
            serenity::ChannelType::Text | serenity::ChannelType::News
        )
    };
    let join_options = options(&view.channels, config.join_log, is_text);
    let leave_options = options(&view.channels, config.leave_log, is_text);
    let total_options = options(&view.channels, config.stats_total, |c| {
        stats::usable(c.kind)
    });
    let online_options = options(&view.channels, config.stats_online, |c| {
        stats::usable(c.kind)
    });
    let owned = if config.stats_category.is_some() {
        "<p class=\"hint\">ห้องสถิติตอนนี้เป็นห้องที่บอทสร้างให้ ถ้าเปลี่ยนเป็นห้องอื่นหรือเลือก \"ไม่ใช้\" ห้องที่บอทสร้างจะถูกลบ</p>"
    } else {
        ""
    };

    let _ = write!(
        body,
        "<form method=\"post\" action=\"/save\" class=\"card\">\
           <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
           <input type=\"hidden\" name=\"guild\" value=\"{guild}\">\
           <h2>แจ้งเตือนคนเข้า/ออก</h2>\
           <div class=\"row\">\
             <label>ห้องแจ้งเตือนคนเข้า<select name=\"join_log\">{join_options}</select></label>\
             <label>ห้องแจ้งเตือนคนออก<select name=\"leave_log\">{leave_options}</select></label>\
           </div>\
           <p class=\"hint\">เลือกคนละห้อง หรือห้องเดียวกันก็ได้ ตอนบันทึก บอทจะส่งข้อความทดสอบเข้าห้องที่เปลี่ยน 1 ครั้ง</p>\
           <h2>ห้องแสดงจำนวน</h2>\
           <p class=\"hint\">เลือกห้องเสียงหรือหมวดหมู่ที่มีอยู่แล้ว บอทจะเปลี่ยนชื่อห้องเป็นตัวเลขล่าสุดทุก 5 นาที \
              ใส่ <code>{{count}}</code> ตรงที่อยากให้แสดงตัวเลข</p>\
           <div class=\"row\">\
             <label>ห้องจำนวนสมาชิก<select name=\"stats_total\">{total_options}</select></label>\
             <label>รูปแบบชื่อ<input name=\"total_name\" maxlength=\"{max}\" value=\"{total_name}\"></label>\
           </div>\
           <div class=\"row\">\
             <label>ห้องคนออนไลน์<select name=\"stats_online\">{online_options}</select></label>\
             <label>รูปแบบชื่อ<input name=\"online_name\" maxlength=\"{max}\" value=\"{online_name}\"></label>\
           </div>\
           {owned}\
           <button type=\"submit\">บันทึก</button>\
         </form>\
         <form method=\"post\" action=\"/create-stats\" class=\"card subtle\">\
           <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
           <input type=\"hidden\" name=\"guild\" value=\"{guild}\">\
           <p>ยังไม่มีห้องที่จะใช้? ให้บอทสร้างหมวดหมู่ \"สถิติเซิร์ฟเวอร์\" พร้อมห้องจำนวนสมาชิกและคนออนไลน์ให้</p>\
           <button type=\"submit\" class=\"secondary\">สร้างห้องใหม่ให้</button>\
         </form>",
        csrf = web.csrf,
        guild = view.guild_id,
        max = stats::MAX_TEMPLATE,
        total_name = esc(config.total_template()),
        online_name = esc(config.online_template()),
    );

    page(&bot_name, &body)
}

fn page(bot_name: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="th">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>จัดการ {name}</title>
<style>
:root {{ --bg:#f5f6f8; --card:#fff; --text:#1f2328; --muted:#636c76; --line:#d8dee4; --accent:#5865f2; --ok:#1a7f37; --err:#cf222e; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#16171a; --card:#1f2124; --text:#e6e7e9; --muted:#9aa0a6; --line:#33363b; --accent:#7983f5; --ok:#3fb950; --err:#f85149; }} }}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--bg); color:var(--text); font:15px/1.6 system-ui, "Segoe UI", "Noto Sans Thai", sans-serif; }}
main {{ max-width:760px; margin:0 auto; padding:24px 16px 48px; }}
h1 {{ font-size:22px; margin:0 0 16px; }}
h2 {{ font-size:16px; margin:20px 0 8px; }}
h2:first-of-type {{ margin-top:0; }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:10px; padding:20px; margin:0 0 16px; }}
.subtle p {{ margin:0 0 12px; color:var(--muted); }}
.stats {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:12px; margin-bottom:16px; }}
.stats div {{ background:var(--card); border:1px solid var(--line); border-radius:10px; padding:12px 14px; }}
.stats span {{ display:block; color:var(--muted); font-size:13px; }}
.stats b {{ font-size:18px; overflow-wrap:anywhere; }}
label {{ display:block; font-weight:600; margin:8px 0; }}
select, input {{ display:block; width:100%; margin-top:4px; padding:8px 10px; font:inherit; color:inherit; background:var(--bg); border:1px solid var(--line); border-radius:6px; }}
.row {{ display:grid; grid-template-columns:1fr 1fr; gap:12px; }}
@media (max-width:560px) {{ .row {{ grid-template-columns:1fr; }} }}
.hint {{ color:var(--muted); font-size:13px; margin:4px 0 8px; }}
code {{ background:var(--bg); padding:1px 5px; border-radius:4px; }}
button {{ margin-top:12px; padding:9px 18px; font:inherit; font-weight:600; color:#fff; background:var(--accent); border:0; border-radius:6px; cursor:pointer; }}
button.secondary {{ margin:0; color:var(--accent); background:transparent; border:1px solid var(--accent); }}
.note {{ padding:10px 14px; border-radius:8px; border:1px solid; margin:0 0 12px; }}
.note.ok {{ color:var(--ok); border-color:var(--ok); }}
.note.err {{ color:var(--err); border-color:var(--err); }}
.guilds {{ display:flex; flex-wrap:wrap; gap:8px; margin-bottom:16px; }}
.guilds a {{ padding:4px 12px; border:1px solid var(--line); border-radius:999px; color:inherit; text-decoration:none; }}
.guilds a.active {{ border-color:var(--accent); color:var(--accent); }}
</style>
</head>
<body><main>
<h1>จัดการบอท {name}</h1>
{body}
</main></body>
</html>"#,
        name = esc(bot_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            esc("<b>\"a\" & 'b'</b>"),
            "&lt;b&gt;&quot;a&quot; &amp; &#39;b&#39;&lt;/b&gt;"
        );
    }

    #[test]
    fn compares_secrets() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "ab"));
    }

    #[test]
    fn parses_form_values() {
        assert_eq!(parse_channel(""), None);
        assert_eq!(parse_channel("0"), None);
        assert_eq!(parse_channel("abc"), None);
        assert_eq!(parse_channel(" 42 "), Some(serenity::ChannelId::new(42)));
        assert_eq!(clean_template("  ", DEFAULT_TOTAL_NAME), None);
        assert_eq!(clean_template(DEFAULT_TOTAL_NAME, DEFAULT_TOTAL_NAME), None);
        assert_eq!(
            clean_template(" คน {count} ", DEFAULT_TOTAL_NAME).as_deref(),
            Some("คน {count}")
        );
    }
}
