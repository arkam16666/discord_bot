//! แกนกลางของระบบเพลง: ตรวจหา yt-dlp, ข้อมูลเพลง, แปล error และตัวรับ event ของห้องเสียง

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use poise::serenity_prelude::{self as serenity, Mentionable, async_trait};
use songbird::events::{Event, EventContext, EventHandler as VoiceEventHandler};
use songbird::input::core::io::{MediaSource, ReadOnlySource};
use songbird::input::{
    AudioStream, AudioStreamError, AuxMetadata, ChildContainer, Compose, Input, YoutubeDl,
};
use songbird::tracks::{LoopState, PlayError, PlayMode, TrackHandle, TrackQueue};
use songbird::{Call, CoreEvent, Songbird, TrackEvent};

use crate::commands::{COLOR, escape, truncate};
use crate::{Context, Error};

/// เพลงในคิวได้สูงสุดกี่เพลงต่อเซิร์ฟเวอร์
pub const MAX_QUEUE: usize = 100;
/// ระดับเสียงเริ่มต้น (1.0 = 100%)
pub const DEFAULT_VOLUME: f32 = 0.5;
/// รอ yt-dlp หาข้อมูลเพลงได้นานสุดเท่าไร
const METADATA_TIMEOUT: Duration = Duration::from_secs(45);
/// รอเชื่อมต่อห้องเสียงได้นานสุดเท่าไร
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);
/// ตรวจสถานะว่างทุกๆ กี่วินาที
const IDLE_CHECK: Duration = Duration::from_secs(30);
/// ว่าง (ไม่มีเพลง หรือไม่มีคนในห้อง) ครบกี่ครั้งถึงออกจากห้อง: 10 x 30 วินาที = 5 นาที
const IDLE_LIMIT: u32 = 10;

/// ชื่อโปรแกรม yt-dlp (ต้องอยู่ใน PATH)
const YTDLP: &str = "yt-dlp";

/// สถานะของระบบเพลงที่ใช้ร่วมกันทั้งบอท
pub struct MusicState {
    http: reqwest::Client,
    /// argument เพิ่มเติมที่ส่งให้ yt-dlp ทุกครั้ง (จาก YTDLP_ARGS และ JavaScript runtime)
    ytdlp_args: Vec<String>,
    ytdlp_ok: AtomicBool,
    /// JavaScript runtime ที่ yt-dlp ใช้ (Deno หรือ Node.js)
    pub js_runtime: Option<&'static str>,
    volumes: Mutex<HashMap<serenity::GuildId, f32>>,
}

impl MusicState {
    /// ตรวจหา yt-dlp และ JavaScript runtime ตอนเริ่มบอท
    pub async fn detect() -> Self {
        let mut ytdlp_args: Vec<String> = std::env::var("YTDLP_ARGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(String::from)
            .collect();

        let ytdlp_ok = match program_version(YTDLP).await {
            Some(v) => {
                println!("[เพลง] พบ yt-dlp เวอร์ชัน {v}");
                true
            }
            None => {
                eprintln!(
                    "[เพลง] ไม่พบ yt-dlp คำสั่งเพลงจะยังใช้ไม่ได้\n\
                     [เพลง] ติดตั้งด้วย: winget install yt-dlp.yt-dlp แล้วเปิดเทอร์มินัลใหม่"
                );
                false
            }
        };

        // yt-dlp ต้องใช้ JavaScript runtime เพื่อดึงเสียงจาก YouTube
        // ค่าเริ่มต้นของ yt-dlp คือ Deno ถ้าไม่มีแต่มี Node.js ให้สั่งใช้ Node แทน
        let js_runtime = if program_version("deno").await.is_some() {
            Some("Deno")
        } else if program_version("node").await.is_some() {
            if !ytdlp_args.iter().any(|a| a.starts_with("--js-runtimes")) {
                ytdlp_args.extend(["--js-runtimes".to_string(), "node".to_string()]);
            }
            Some("Node.js")
        } else {
            eprintln!(
                "[เพลง] ไม่พบ Deno หรือ Node.js ซึ่ง yt-dlp ต้องใช้กับ YouTube\n\
                 [เพลง] ติดตั้งด้วย: winget install DenoLand.Deno"
            );
            None
        };
        if let Some(name) = js_runtime {
            println!("[เพลง] ใช้ {name} เป็น JavaScript runtime");
        }

        Self {
            http: reqwest::Client::new(),
            ytdlp_args,
            ytdlp_ok: AtomicBool::new(ytdlp_ok),
            js_runtime,
            volumes: Mutex::new(HashMap::new()),
        }
    }

    pub fn ytdlp_ready(&self) -> bool {
        self.ytdlp_ok.load(Ordering::Relaxed)
    }

    /// เช็คว่า yt-dlp ใช้ได้ ถ้าตอนเริ่มบอทหาไม่เจอจะลองหาใหม่ (เผื่อเพิ่งติดตั้ง)
    pub async fn ensure_ytdlp(&self) -> Result<(), Error> {
        if self.ytdlp_ready() {
            return Ok(());
        }
        if program_version(YTDLP).await.is_some() {
            self.ytdlp_ok.store(true, Ordering::Relaxed);
            return Ok(());
        }
        Err("ไม่พบโปรแกรม yt-dlp บนเครื่องที่รันบอท ให้ผู้ดูแลติดตั้งด้วย `winget install yt-dlp.yt-dlp`".into())
    }

    /// สร้างตัวดึงเสียงจาก YouTube ด้วยลิงก์
    pub fn ytdl_url(&self, url: String) -> YoutubeDl<'static> {
        YoutubeDl::new_ytdl_like(YTDLP, self.http.clone(), url).user_args(self.ytdlp_args.clone())
    }

    /// สร้างตัวค้นหาเพลงบน YouTube
    pub fn ytdl_search(&self, query: String) -> YoutubeDl<'static> {
        YoutubeDl::new_search_ytdl_like(YTDLP, self.http.clone(), query)
            .user_args(self.ytdlp_args.clone())
    }

    /// เสียงของเพลงสำหรับเล่นจริง: ให้ yt-dlp ดาวน์โหลดแล้วส่งเสียงต่อมาทาง stdout
    /// (ถ้าเอาลิงก์ไฟล์เสียงที่ yt-dlp หาให้ไปดึงเอง YouTube จะตอบ 403 Forbidden)
    pub fn stream(&self, url: String) -> Input {
        Input::Lazy(Box::new(YtDlpStream {
            url,
            args: self.ytdlp_args.clone(),
        }))
    }

    pub fn volume(&self, guild: serenity::GuildId) -> f32 {
        let map = self.volumes.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&guild).copied().unwrap_or(DEFAULT_VOLUME)
    }

    pub fn set_volume(&self, guild: serenity::GuildId, volume: f32) {
        let mut map = self.volumes.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(guild, volume);
    }

    /// ดึงข้อมูลเพลงด้วย yt-dlp พร้อม timeout และแปล error เป็นภาษาไทย
    pub async fn fetch_metadata(
        &self,
        ytdl: &mut YoutubeDl<'static>,
    ) -> Result<AuxMetadata, Error> {
        match tokio::time::timeout(METADATA_TIMEOUT, ytdl.aux_metadata()).await {
            Err(_) => Err("YouTube ตอบช้าเกินไป ลองใหม่อีกครั้ง".into()),
            Ok(Err(e)) => {
                if e.to_string().contains("could not find executable") {
                    self.ytdlp_ok.store(false, Ordering::Relaxed);
                }
                Err(explain_stream_error(&e).into())
            }
            Ok(Ok(meta)) => Ok(meta),
        }
    }
}

/// เปิด yt-dlp ตอนเพลงเริ่มเล่นจริง (ไม่ใช่ตอนเข้าคิว) และปิดเองเมื่อเพลงจบหรือถูกข้าม
struct YtDlpStream {
    url: String,
    args: Vec<String>,
}

#[async_trait]
impl Compose for YtDlpStream {
    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let child = std::process::Command::new(YTDLP)
            .args(&self.args)
            // เลือกเสียง opus ก่อน เพราะเป็นรูปแบบเดียวกับที่ Discord ใช้
            .args(["--quiet", "--no-warnings", "--no-playlist"])
            .args(["-f", "ba[acodec=opus]/ba/b", "-o", "-", "--"])
            .arg(&self.url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // error ของ yt-dlp (ถ้ามี) จะแสดงใน log ของบอท
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| AudioStreamError::Fail(format!("เปิด yt-dlp ไม่ได้: {e}").into()))?;
        Ok(AudioStream {
            input: Box::new(ReadOnlySource::new(ChildContainer::from(child))),
        })
    }

    async fn create_async(
        &mut self,
    ) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        Err(AudioStreamError::Unsupported)
    }

    fn should_create_async(&self) -> bool {
        false
    }
}

/// รันโปรแกรมด้วย --version เพื่อเช็คว่ามีอยู่จริง (timeout 15 วินาที)
async fn program_version(program: &str) -> Option<String> {
    let output = tokio::process::Command::new(program)
        .arg("--version")
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(15), output).await {
        Ok(Ok(out)) if out.status.success() => Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
        _ => None,
    }
}

/// ข้อมูลของเพลงที่แนบไว้กับแต่ละ track
pub struct TrackMeta {
    pub title: String,
    pub url: String,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
    pub channel: Option<String>,
    pub requester: serenity::UserId,
}

impl TrackMeta {
    pub fn from_aux(aux: AuxMetadata, requester: serenity::UserId) -> Result<Self, Error> {
        let url = aux
            .source_url
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .ok_or("ไม่พบลิงก์ของวิดีโอนี้ ลองใช้ลิงก์ตรงของวิดีโอแทน")?;
        Ok(Self {
            title: aux
                .title
                .or(aux.track)
                .unwrap_or_else(|| "ไม่ทราบชื่อเพลง".to_string()),
            url,
            duration: aux.duration.filter(|d| !d.is_zero()),
            thumbnail: aux.thumbnail,
            channel: aux.channel.or(aux.artist),
            requester,
        })
    }

    /// ชื่อเพลงพร้อมลิงก์ ใช้ใน embed
    pub fn link(&self) -> String {
        format!("[{}]({})", escape(&truncate(&self.title, 80)), self.url)
    }

    pub fn duration_text(&self) -> String {
        self.duration
            .map(format_time)
            .unwrap_or_else(|| "ไลฟ์สด".to_string())
    }
}

/// อ่านข้อมูลเพลงจาก track (ทุก track ของบอทสร้างพร้อม TrackMeta เสมอ)
pub fn meta(handle: &TrackHandle) -> Arc<TrackMeta> {
    handle.data::<TrackMeta>()
}

/// แปลงเวลาเป็น m:ss หรือ h:mm:ss
pub fn format_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// สิ่งที่ผู้ใช้ส่งมาในคำสั่ง play
pub enum Source {
    Url(String),
    Search(String),
}

/// ตรวจและแยกว่าเป็นลิงก์ YouTube หรือคำค้นหา
pub fn parse_source(input: &str) -> Result<Source, Error> {
    // Discord ใช้ <ลิงก์> เพื่อซ่อนตัวอย่างลิงก์ ให้ตัดวงเล็บออก
    let input = input
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    if input.is_empty() {
        return Err("ใส่ลิงก์ YouTube หรือชื่อเพลงที่ต้องการค้นหา".into());
    }
    if input.chars().count() > 300 {
        return Err("ข้อความยาวเกินไป (ไม่เกิน 300 ตัวอักษร)".into());
    }

    let lower = input.to_lowercase();
    let looks_like_url = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("www.")
        || (!input.contains(char::is_whitespace)
            && (lower.contains("youtube.com/") || lower.contains("youtu.be/")));
    if !looks_like_url {
        return Ok(Source::Search(input.to_string()));
    }

    let url = if lower.starts_with("http://") || lower.starts_with("https://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    if url.contains(char::is_whitespace) {
        return Err("ลิงก์ไม่ถูกต้อง".into());
    }

    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let is_youtube = host == "youtube.com" || host == "youtu.be" || host.ends_with(".youtube.com");
    if !is_youtube {
        return Err("รองรับเฉพาะลิงก์จาก YouTube (หรือพิมพ์ชื่อเพลงเพื่อค้นหา)".into());
    }

    let is_single_video = host == "youtu.be"
        || url.contains("v=")
        || url.contains("/shorts/")
        || url.contains("/live/")
        || url.contains("/embed/");
    if !is_single_video {
        if url.contains("list=") {
            return Err("ยังไม่รองรับเพลย์ลิสต์ ใช้ลิงก์ของวิดีโอทีละเพลง".into());
        }
        return Err("ลิงก์นี้ไม่ใช่ลิงก์วิดีโอ ใช้ลิงก์ที่เปิดวิดีโอโดยตรง".into());
    }
    Ok(Source::Url(url))
}

/// แปล error ตอนดึงเสียงให้อ่านเข้าใจ
pub fn explain_stream_error(err: &AudioStreamError) -> String {
    let raw = err.to_string();
    let lower = raw.to_lowercase();
    let has = |s: &str| lower.contains(&s.to_lowercase());

    if has("could not find executable") {
        "ไม่พบโปรแกรม yt-dlp บนเครื่องที่รันบอท".to_string()
    } else if has("no results found") {
        "ไม่พบเพลงที่ค้นหา ลองใช้คำอื่น".to_string()
    } else if has("confirm your age") || has("age-restricted") || has("age restricted") {
        "วิดีโอนี้จำกัดอายุ บอทเล่นไม่ได้".to_string()
    } else if has("private video") {
        "วิดีโอนี้เป็นส่วนตัว".to_string()
    } else if has("not a bot") {
        "YouTube บล็อกการดึงข้อมูลชั่วคราว ลองใหม่ภายหลัง หรือตั้งค่า cookies ผ่าน YTDLP_ARGS".to_string()
    } else if has("javascript runtime") || has("js-runtimes") || has("nsig") {
        "yt-dlp ต้องใช้ Deno หรือ Node.js ในการดึงเสียงจาก YouTube ให้ผู้ดูแลติดตั้งเพิ่ม".to_string()
    } else if has("members-only") || has("join this channel") {
        "วิดีโอนี้สำหรับสมาชิกช่องเท่านั้น".to_string()
    } else if has("will begin") || has("premieres in") {
        "ไลฟ์หรือพรีเมียร์นี้ยังไม่เริ่ม".to_string()
    } else if has("not available in your country") || has("geo restrict") {
        "วิดีโอนี้ถูกจำกัดประเทศ".to_string()
    } else if has("requested format is not available") {
        "ไม่พบรูปแบบเสียงที่บอทเล่นได้ (มักเกิดกับไลฟ์สด หรือ yt-dlp ต้องอัปเดตด้วย `yt-dlp -U`)".to_string()
    } else if has("video unavailable") || has("is not available") || has("has been removed") {
        "วิดีโอนี้ไม่พร้อมใช้งาน (อาจถูกลบหรือถูกจำกัด)".to_string()
    } else if has("unsupported url") {
        "ลิงก์นี้ไม่รองรับ".to_string()
    } else if has("http error 429") || has("too many requests") {
        "ส่งคำขอไปที่ YouTube ถี่เกินไป รอสักครู่แล้วลองใหม่".to_string()
    } else if has("update yt-dlp") || has("nightly") {
        "yt-dlp เก่าเกินไป ให้ผู้ดูแลอัปเดตด้วย `yt-dlp -U`".to_string()
    } else {
        // หาบรรทัด ERROR: ของ yt-dlp ถ้ามี
        let detail = raw
            .lines()
            .find(|l| l.contains("ERROR:"))
            .unwrap_or(&raw)
            .trim();
        format!("ดึงเสียงไม่สำเร็จ: {}", truncate(detail, 300))
    }
}

/// แปล error ระหว่างเล่นเพลง
fn explain_play_error(err: &PlayError) -> String {
    match err {
        PlayError::Create(e) => explain_stream_error(e),
        PlayError::Parse(_) => {
            "อ่านเสียงไม่ได้ yt-dlp อาจดาวน์โหลดไม่สำเร็จ (ดูรายละเอียดใน log ของบอท)".to_string()
        }
        PlayError::Decode(_) => "ถอดรหัสเสียงไม่สำเร็จ".to_string(),
        other => format!("เล่นไม่สำเร็จ: {}", truncate(&other.to_string(), 300)),
    }
}

/// แปล error ตอนเข้าห้องเสียง
fn explain_join_error(err: &songbird::error::JoinError) -> String {
    use songbird::error::JoinError as J;
    match err {
        J::TimedOut => "เชื่อมต่อห้องเสียงไม่ทัน ลองใหม่อีกครั้ง".to_string(),
        J::Dropped | J::NoSender => "การเชื่อมต่อกับ Discord หลุด ลองใหม่อีกครั้ง".to_string(),
        other => format!("เข้าห้องเสียงไม่สำเร็จ: {}", truncate(&other.to_string(), 300)),
    }
}

/// ห้องเสียงที่ผู้ใช้อยู่ตอนนี้
fn voice_channel_of(
    cache: &serenity::Cache,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> Result<serenity::ChannelId, Error> {
    let guild = cache
        .guild(guild_id)
        .ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache ลองใหม่อีกครั้ง")?;
    guild
        .voice_states
        .get(&user_id)
        .and_then(|vs| vs.channel_id)
        .ok_or_else(|| "คุณต้องเข้าห้องเสียงก่อน".into())
}

pub async fn voice_manager(ctx: &serenity::Context) -> Result<Arc<Songbird>, Error> {
    songbird::get(ctx)
        .await
        .ok_or_else(|| "ระบบเสียงยังไม่พร้อมใช้งาน".into())
}

/// ให้บอทเข้าห้องเสียงเดียวกับผู้ใช้ (ถ้าอยู่แล้วก็ใช้ห้องเดิม)
pub async fn join_author(ctx: Context<'_>) -> Result<Arc<tokio::sync::Mutex<Call>>, Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let target = voice_channel_of(ctx.cache(), guild_id, ctx.author().id)?;
    let manager = voice_manager(ctx.serenity_context()).await?;

    if let Some(call) = manager.get(guild_id) {
        let handler = call.lock().await;
        match handler.current_channel() {
            Some(current) if current == target.into() => return Ok(call.clone()),
            Some(_) if !handler.queue().is_empty() => {
                return Err("บอทกำลังเล่นเพลงอยู่ในห้องอื่น เข้าห้องเดียวกับบอทหรือใช้ /stop ก่อน".into());
            }
            _ => {}
        }
    }

    // เช็คสิทธิ์ของบอทในห้องเสียงก่อนเข้า
    let bot_id = ctx.framework().bot_id;
    let perms = {
        let guild = ctx.guild().ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache")?;
        match (guild.channels.get(&target), guild.members.get(&bot_id)) {
            (Some(channel), Some(member)) => Some(guild.user_permissions_in(channel, member)),
            _ => None,
        }
    };
    if let Some(perms) = perms {
        if !perms.view_channel() || !perms.connect() {
            return Err("บอทไม่มีสิทธิ์เข้าห้องเสียงนี้ (ต้องมีสิทธิ์ View Channel และ Connect)".into());
        }
        if !perms.speak() {
            return Err("บอทไม่มีสิทธิ์พูดในห้องเสียงนี้ (ต้องมีสิทธิ์ Speak)".into());
        }
    }

    let call = match tokio::time::timeout(JOIN_TIMEOUT, manager.join(guild_id, target)).await {
        Ok(Ok(call)) => call,
        Ok(Err(e)) => {
            let _ = manager.remove(guild_id).await;
            return Err(explain_join_error(&e).into());
        }
        Err(_) => {
            let _ = manager.remove(guild_id).await;
            return Err("เชื่อมต่อห้องเสียงไม่ทัน ลองใหม่อีกครั้ง".into());
        }
    };

    {
        let mut handler = call.lock().await;
        // ปิดหูบอท ประหยัดแบนด์วิดท์ (ถ้าไม่สำเร็จก็ไม่เป็นไร)
        let _ = handler.deafen(true).await;

        let serenity_ctx = ctx.serenity_context();
        let http = serenity_ctx.http.clone();
        let text_channel = ctx.channel_id();
        let queue = handler.queue().clone();

        // ล้าง event เก่า (ถ้าย้ายห้อง) แล้วลงทะเบียนใหม่
        handler.remove_all_global_events();
        handler.add_global_event(
            Event::Track(TrackEvent::Play),
            TrackNotifier::new(http.clone(), text_channel),
        );
        handler.add_global_event(
            Event::Track(TrackEvent::Error),
            TrackNotifier::new(http.clone(), text_channel),
        );
        handler.add_global_event(
            Event::Core(CoreEvent::DriverDisconnect),
            DisconnectCleanup {
                manager: manager.clone(),
                http: http.clone(),
                text_channel,
                queue: queue.clone(),
            },
        );
        handler.add_global_event(
            Event::Periodic(IDLE_CHECK, None),
            IdleWatcher {
                manager: manager.clone(),
                cache: serenity_ctx.cache.clone(),
                http,
                guild_id,
                bot_id,
                text_channel,
                queue,
                idle_ticks: AtomicU32::new(0),
            },
        );
    }

    Ok(call)
}

/// คิวเพลงที่ผู้ใช้ควบคุมได้: บอทต้องอยู่ในห้องเสียง และผู้ใช้ต้องอยู่ห้องเดียวกัน
async fn queue_for(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> Result<TrackQueue, Error> {
    let manager = voice_manager(ctx).await?;
    let call = manager.get(guild_id).ok_or("บอทยังไม่ได้อยู่ในห้องเสียง")?;
    let handler = call.lock().await;
    let bot_channel = handler.current_channel().ok_or("บอทยังไม่ได้อยู่ในห้องเสียง")?;
    if bot_channel != voice_channel_of(&ctx.cache, guild_id, user_id)?.into() {
        return Err("คุณต้องอยู่ห้องเสียงเดียวกับบอท".into());
    }
    Ok(handler.queue().clone())
}

/// ใช้กับคำสั่งควบคุมเพลง
pub async fn require_same_channel(ctx: Context<'_>) -> Result<TrackQueue, Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    queue_for(ctx.serenity_context(), guild_id, ctx.author().id).await
}

fn current_track(queue: &TrackQueue) -> Result<TrackHandle, Error> {
    queue.current().ok_or_else(|| "ตอนนี้ไม่มีเพลงที่กำลังเล่น".into())
}

/// หยุดชั่วคราว ถ้าหยุดอยู่แล้วให้เล่นต่อ
pub async fn toggle_pause(queue: &TrackQueue) -> Result<String, Error> {
    let current = current_track(queue)?;
    if matches!(current.get_info().await?.playing, PlayMode::Pause) {
        queue.resume()?;
        Ok("เล่นเพลงต่อแล้ว".to_string())
    } else {
        queue.pause()?;
        Ok("หยุดเพลงชั่วคราวแล้ว".to_string())
    }
}

pub fn skip(queue: &TrackQueue) -> Result<String, Error> {
    let title = meta(&current_track(queue)?).title.clone();
    queue.skip()?;
    let end = if queue.len() > 1 {
        ""
    } else {
        " (หมดคิวแล้ว)"
    };
    Ok(format!("ข้ามเพลง **{}** แล้ว{end}", escape(&title)))
}

pub fn stop(queue: &TrackQueue) -> Result<String, Error> {
    if queue.is_empty() {
        return Err("ตอนนี้ไม่มีเพลงในคิว".into());
    }
    queue.stop();
    Ok("หยุดเล่นและล้างคิวแล้ว".to_string())
}

/// เปิด/ปิดการวนเล่นเพลงปัจจุบัน
pub async fn toggle_loop(queue: &TrackQueue) -> Result<String, Error> {
    let current = current_track(queue)?;
    if is_looping(&current.get_info().await?.loops) {
        current.disable_loop()?;
        Ok("ปิดวนเล่นแล้ว".to_string())
    } else {
        current.enable_loop()?;
        Ok("เปิดวนเล่นเพลงนี้แล้ว".to_string())
    }
}

pub fn is_looping(loops: &LoopState) -> bool {
    !matches!(loops, LoopState::Finite(n) if n.get() == 0)
}

/// embed แสดงคิวเพลง
pub fn queue_embed(queue: &TrackQueue) -> Result<serenity::CreateEmbed, Error> {
    let tracks = queue.current_queue();
    let Some((current, upcoming)) = tracks.split_first() else {
        return Err("คิวว่างอยู่ ใช้ /play เพื่อเพิ่มเพลง".into());
    };

    let now = meta(current);
    let mut text = format!("**กำลังเล่น:** {} [{}]\n", now.link(), now.duration_text());
    if upcoming.is_empty() {
        text.push_str("\nไม่มีเพลงถัดไป");
    } else {
        text.push_str("\n**ถัดไป:**\n");
        for (i, handle) in upcoming.iter().take(10).enumerate() {
            let m = meta(handle);
            text.push_str(&format!(
                "`{}.` {} [{}]\n",
                i + 1,
                m.link(),
                m.duration_text()
            ));
        }
        if upcoming.len() > 10 {
            text.push_str(&format!("...และอีก {} เพลง\n", upcoming.len() - 10));
        }
    }

    let total: Duration = tracks.iter().filter_map(|h| meta(h).duration).sum();
    Ok(serenity::CreateEmbed::new()
        .title("คิวเพลง")
        .description(truncate(&text, 4000))
        .color(COLOR)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "{} เพลง • รวม {}",
            tracks.len(),
            format_time(total)
        ))))
}

/// ปุ่มควบคุมเพลงที่แนบไปกับข้อความ "กำลังเล่น"
pub fn controls() -> Vec<serenity::CreateActionRow> {
    use serenity::{ButtonStyle, CreateButton};
    vec![serenity::CreateActionRow::Buttons(vec![
        CreateButton::new("music:pause")
            .label("หยุด/เล่นต่อ")
            .style(ButtonStyle::Secondary),
        CreateButton::new("music:skip")
            .label("ข้าม")
            .style(ButtonStyle::Primary),
        CreateButton::new("music:loop")
            .label("วนซ้ำ")
            .style(ButtonStyle::Secondary),
        CreateButton::new("music:queue")
            .label("คิว")
            .style(ButtonStyle::Secondary),
        CreateButton::new("music:stop")
            .label("หยุดทั้งหมด")
            .style(ButtonStyle::Danger),
    ])]
}

/// จัดการเมื่อมีคนกดปุ่มควบคุมเพลง
pub async fn handle_button(
    ctx: &serenity::Context,
    press: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    use serenity::{CreateInteractionResponse, CreateInteractionResponseMessage as Msg};

    let Some(action) = press.data.custom_id.strip_prefix("music:") else {
        return Ok(());
    };

    let result: Result<Msg, Error> = async {
        let guild_id = press.guild_id.ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
        let queue = queue_for(ctx, guild_id, press.user.id).await?;
        let text = match action {
            "pause" => toggle_pause(&queue).await?,
            "skip" => skip(&queue)?,
            "stop" => stop(&queue)?,
            "loop" => toggle_loop(&queue).await?,
            "queue" => return Ok(Msg::new().embed(queue_embed(&queue)?).ephemeral(true)),
            _ => return Err("ปุ่มนี้ใช้ไม่ได้แล้ว".into()),
        };
        // แจ้งให้ทุกคนเห็นว่าใครกด (แสดงชื่อแต่ไม่แท็ก)
        Ok(Msg::new()
            .content(format!("{text} (โดย {})", press.user.mention()))
            .allowed_mentions(serenity::CreateAllowedMentions::new()))
    }
    .await;

    let message = result.unwrap_or_else(|e| Msg::new().content(e.to_string()).ephemeral(true));
    press
        .create_response(ctx, CreateInteractionResponse::Message(message))
        .await?;
    Ok(())
}

/// ส่งข้อความแจ้งเตือนลงช่อง (ไม่แท็กใคร) ถ้าส่งไม่ได้ก็แค่ log ไว้
async fn notify(
    http: &serenity::Http,
    channel: serenity::ChannelId,
    message: serenity::CreateMessage,
) {
    let message = message.allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Err(e) = channel.send_message(http, message).await {
        eprintln!("[เพลง] ส่งข้อความแจ้งเตือนไม่สำเร็จ: {e}");
    }
}

fn text(content: impl Into<String>) -> serenity::CreateMessage {
    serenity::CreateMessage::new().content(content)
}

/// embed "กำลังเล่น" ของเพลง
pub fn now_playing_embed(meta: &TrackMeta, title: &str) -> serenity::CreateEmbed {
    let mut embed = serenity::CreateEmbed::new()
        .title(title)
        .description(meta.link())
        .color(COLOR)
        .field("ความยาว", meta.duration_text(), true)
        .field("ขอโดย", format!("<@{}>", meta.requester), true);
    if let Some(channel) = &meta.channel {
        embed = embed.field("ช่อง", escape(&truncate(channel, 100)), true);
    }
    if let Some(thumb) = &meta.thumbnail {
        embed = embed.thumbnail(thumb);
    }
    embed
}

/// แจ้งในช่องข้อความเมื่อเพลงถัดไปเริ่มเล่น หรือเมื่อเล่นไม่สำเร็จ
struct TrackNotifier {
    http: Arc<serenity::Http>,
    channel: serenity::ChannelId,
    last_announced: Mutex<Option<u128>>,
}

impl TrackNotifier {
    fn new(http: Arc<serenity::Http>, channel: serenity::ChannelId) -> Self {
        Self {
            http,
            channel,
            last_announced: Mutex::new(None),
        }
    }
}

#[async_trait]
impl VoiceEventHandler for TrackNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let EventContext::Track(tracks) = ctx else {
            return None;
        };
        for (state, handle) in tracks.iter() {
            let meta = meta(handle);
            match &state.playing {
                PlayMode::Errored(err) => {
                    let reason = explain_play_error(err);
                    eprintln!("[เพลง] เล่น {} ไม่สำเร็จ: {err}", meta.title);
                    notify(
                        &self.http,
                        self.channel,
                        text(format!(
                            "เล่นเพลง **{}** ไม่สำเร็จ: {reason}",
                            escape(&meta.title)
                        )),
                    )
                    .await;
                }
                // เพลงถัดไปในคิวเริ่มเล่น (ไม่แจ้งตอนกด resume กลางเพลง)
                PlayMode::Play if state.position < Duration::from_secs(1) => {
                    let id = handle.uuid().as_u128();
                    let is_new = {
                        let mut last = self
                            .last_announced
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let is_new = *last != Some(id);
                        *last = Some(id);
                        is_new
                    };
                    if is_new {
                        let message = serenity::CreateMessage::new()
                            .embed(now_playing_embed(&meta, "กำลังเล่น"))
                            .components(controls());
                        notify(&self.http, self.channel, message).await;
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// เมื่อบอทหลุดจากห้องเสียง (เช่น ถูกเตะออก) ให้ล้างคิวและคืนทรัพยากร
struct DisconnectCleanup {
    manager: Arc<Songbird>,
    http: Arc<serenity::Http>,
    text_channel: serenity::ChannelId,
    queue: TrackQueue,
}

#[async_trait]
impl VoiceEventHandler for DisconnectCleanup {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let EventContext::DriverDisconnect(data) = ctx else {
            return None;
        };
        // บอทออกเองด้วยคำสั่ง ไม่ต้องทำอะไร
        if matches!(
            data.reason,
            Some(songbird::events::context_data::DisconnectReason::Requested)
        ) {
            return None;
        }

        eprintln!(
            "[เพลง] หลุดจากห้องเสียงในเซิร์ฟเวอร์ {}: {:?}",
            data.guild_id.0, data.reason
        );
        self.queue.stop();
        let guild_id = serenity::GuildId::new(data.guild_id.0.get());
        let manager = self.manager.clone();
        // ลบ Call นอก event handler เพื่อไม่ให้ค้างกัน
        tokio::spawn(async move {
            let _ = manager.remove(guild_id).await;
        });
        notify(
            &self.http,
            self.text_channel,
            text("หลุดจากห้องเสียง จึงล้างคิวเพลงแล้ว ใช้ /play เพื่อเริ่มใหม่"),
        )
        .await;
        None
    }
}

/// ออกจากห้องเสียงอัตโนมัติเมื่อไม่มีเพลงหรือไม่มีคนฟังครบ 5 นาที
struct IdleWatcher {
    manager: Arc<Songbird>,
    cache: Arc<serenity::Cache>,
    http: Arc<serenity::Http>,
    guild_id: serenity::GuildId,
    bot_id: serenity::UserId,
    text_channel: serenity::ChannelId,
    queue: TrackQueue,
    idle_ticks: AtomicU32,
}

impl IdleWatcher {
    /// ไม่มีคน (ที่ไม่ใช่บอท) อยู่ในห้องเดียวกับบอท
    fn is_alone(&self) -> bool {
        let Some(guild) = self.cache.guild(self.guild_id) else {
            return false;
        };
        let Some(bot_channel) = guild
            .voice_states
            .get(&self.bot_id)
            .and_then(|vs| vs.channel_id)
        else {
            return false;
        };
        !guild.voice_states.values().any(|vs| {
            vs.channel_id == Some(bot_channel)
                && vs.user_id != self.bot_id
                && !vs.member.as_ref().is_some_and(|m| m.user.bot)
        })
    }
}

#[async_trait]
impl VoiceEventHandler for IdleWatcher {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        let idle = self.queue.is_empty() || self.is_alone();
        if !idle {
            self.idle_ticks.store(0, Ordering::Relaxed);
            return None;
        }
        let ticks = self.idle_ticks.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks < IDLE_LIMIT {
            return None;
        }

        self.queue.stop();
        let manager = self.manager.clone();
        let guild_id = self.guild_id;
        tokio::spawn(async move {
            let _ = manager.remove(guild_id).await;
        });
        notify(
            &self.http,
            self.text_channel,
            text("ออกจากห้องเสียงแล้ว เพราะไม่มีการใช้งานครบ 5 นาที"),
        )
        .await;
        Some(Event::Cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ทดสอบเล่นเสียงจาก YouTube จริง (ต้องมี yt-dlp และอินเทอร์เน็ต): cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn streams_real_youtube_audio() {
        use songbird::input::LiveInput;
        use songbird::input::codecs::{get_codec_registry, get_probe};

        let state = MusicState::detect().await;
        let input = state.stream("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into());
        let playable = input
            .make_playable_async(get_codec_registry(), get_probe())
            .await
            .expect("yt-dlp ควรส่งเสียงที่อ่านได้");
        let Input::Live(LiveInput::Parsed(mut parsed), _) = playable else {
            panic!("ควรได้ stream ที่พร้อมเล่น");
        };
        // ถอดรหัสเสียงได้จริงอย่างน้อย 50 packet (~1 วินาที)
        for _ in 0..50 {
            let packet = parsed.format.next_packet().expect("อ่าน packet ได้");
            if packet.track_id() == parsed.track_id {
                parsed.decoder.decode(&packet).expect("ถอดรหัสได้");
            }
        }
    }

    fn url(input: &str) -> String {
        match parse_source(input) {
            Ok(Source::Url(u)) => u,
            Ok(Source::Search(s)) => panic!("expected url, got search: {s}"),
            Err(e) => panic!("expected url, got error: {e}"),
        }
    }

    fn search(input: &str) -> String {
        match parse_source(input) {
            Ok(Source::Search(s)) => s,
            Ok(Source::Url(u)) => panic!("expected search, got url: {u}"),
            Err(e) => panic!("expected search, got error: {e}"),
        }
    }

    #[test]
    fn accepts_youtube_video_links() {
        url("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        url("https://youtu.be/dQw4w9WgXcQ");
        url("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD123");
        url("https://m.youtube.com/watch?v=dQw4w9WgXcQ");
        url("https://www.youtube.com/shorts/abc123");
        assert_eq!(url("<https://youtu.be/x>"), "https://youtu.be/x");
        assert_eq!(url("youtu.be/x"), "https://youtu.be/x");
        assert_eq!(
            url("www.youtube.com/watch?v=x"),
            "https://www.youtube.com/watch?v=x"
        );
    }

    #[test]
    fn treats_plain_text_as_search() {
        assert_eq!(search("  ลาลาลอย  "), "ลาลาลอย");
        assert_eq!(search("never gonna give you up"), "never gonna give you up");
        // ข้อความที่ขึ้นต้นด้วย - ต้องเป็นคำค้นหา ไม่ใช่ option ของ yt-dlp
        assert_eq!(search("--exec calc"), "--exec calc");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_source("").is_err());
        assert!(parse_source("   ").is_err());
        assert!(parse_source(&"a".repeat(301)).is_err());
        assert!(parse_source("https://example.com/song.mp3").is_err());
        assert!(parse_source("https://evil-youtube.com/watch?v=x").is_err());
        assert!(parse_source("https://youtube.com.evil.com/watch?v=x").is_err());
        assert!(parse_source("https://www.youtube.com/playlist?list=PL123").is_err());
        assert!(parse_source("https://www.youtube.com/@channel").is_err());
    }

    #[test]
    fn explains_live_stream_format_error() {
        let err = AudioStreamError::Fail(
            "yt-dlp failed: ERROR: [youtube] abc: Requested format is not available.".into(),
        );
        assert!(explain_stream_error(&err).contains("ไลฟ์สด"));
        let err = AudioStreamError::Fail("ERROR: [youtube] abc: Video unavailable".into());
        assert!(explain_stream_error(&err).contains("ไม่พร้อมใช้งาน"));
    }

    #[test]
    fn formats_time() {
        assert_eq!(format_time(Duration::from_secs(5)), "0:05");
        assert_eq!(format_time(Duration::from_secs(185)), "3:05");
        assert_eq!(format_time(Duration::from_secs(3725)), "1:02:05");
    }
}
