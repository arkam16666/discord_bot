mod commands;
mod error;
mod events;
mod members;
mod music;
mod settings;
mod stats;
mod web;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use poise::serenity_prelude as serenity;
use songbird::SerenityInit;

use members::Features;

/// ข้อมูลที่แชร์ให้ทุกคำสั่งใช้ร่วมกัน (เข้าถึงได้ผ่าน `ctx.data()`)
pub struct Data {
    /// เวลาที่บอทเริ่มทำงาน ใช้คำนวณ uptime
    pub start_time: Instant,
    /// prefix ของคำสั่งแบบพิมพ์ เช่น "!"
    pub prefix: String,
    /// ฟีเจอร์ที่ใช้ได้ตาม intent ที่เปิดไว้
    pub features: Features,
    /// ระบบเพลง
    pub music: Arc<music::MusicState>,
    /// การตั้งค่าของแต่ละเซิร์ฟเวอร์
    pub settings: Arc<settings::Settings>,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// ค่าที่โหลดครั้งเดียวตอนเริ่มบอท
struct Config {
    start_time: Instant,
    prefix: String,
    music: Arc<music::MusicState>,
    settings: Arc<settings::Settings>,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[ผิดพลาด] {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Error> {
    if let Err(e) = dotenvy::dotenv() {
        // ไม่มีไฟล์ .env ไม่เป็นไร (อาจตั้งค่าผ่าน environment ตรงๆ) แต่ไฟล์เสียต้องแจ้ง
        if !e.not_found() {
            eprintln!("[คำเตือน] อ่านไฟล์ .env ไม่สำเร็จ: {e}");
        }
    }

    let token = std::env::var("DISCORD_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or(
            "ไม่พบ DISCORD_TOKEN ใน .env\n\
             ถ้าใช้ Docker ให้เปิดด้วยคำสั่ง docker compose up -d ในโฟลเดอร์บอท \
             (docker run หรือปุ่ม Run ใน Docker Desktop จะไม่ส่งค่าจาก .env เข้าไป)",
        )?;
    serenity::validate_token(&token)
        .map_err(|_| "DISCORD_TOKEN ไม่ถูกต้อง ตรวจสอบว่าคัดลอก token มาครบ")?;

    let config = Arc::new(Config {
        start_time: Instant::now(),
        prefix: std::env::var("PREFIX")
            .ok()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "!".to_string()),
        music: Arc::new(music::MusicState::detect().await),
        settings: Arc::new(settings::Settings::load("data/settings.json")),
    });

    // ลองเชื่อมต่อด้วยฟีเจอร์ครบก่อน ถ้า Discord ไม่อนุญาต intent ไหน (ยังไม่ได้เปิดใน Developer Portal)
    // ให้ลดฟีเจอร์ลงทีละขั้น บอทจะได้ยังทำงานได้แทนที่จะปิดตัวไปเลย
    let tiers = [
        (true, true, true),
        (true, false, true),
        (false, false, true),
        (false, false, false),
    ]
    .map(|(members, presences, message_content)| Features {
        members,
        presences,
        message_content,
    });
    for (i, features) in tiers.into_iter().enumerate() {
        match start_client(&token, features, config.clone()).await {
            Ok(()) => return Ok(()),
            Err(e)
                if i + 1 < tiers.len()
                    && matches!(
                        *e,
                        serenity::Error::Gateway(serenity::GatewayError::DisallowedGatewayIntents)
                    ) =>
            {
                eprintln!("[คำเตือน] Discord ไม่อนุญาต intent บางตัว จะลองเชื่อมต่อใหม่โดยปิดบางฟีเจอร์");
            }
            Err(e) => return Err(explain_client_error(&e).into()),
        }
    }
    Err("เชื่อมต่อ Discord ไม่สำเร็จ".into())
}

/// พิมพ์บอกว่าฟีเจอร์ไหนปิดอยู่ และต้องเปิด intent อะไร
fn report_features(features: Features) {
    let mut off = Vec::new();
    if !features.presences {
        off.push("นับคนออนไลน์ (เปิด PRESENCE INTENT)");
    }
    if !features.members {
        off.push("แจ้งเตือนคนเข้า/ออก (เปิด SERVER MEMBERS INTENT)");
    }
    if !features.message_content {
        off.push("คำสั่งแบบ prefix (เปิด MESSAGE CONTENT INTENT)");
    }
    if off.is_empty() {
        println!("[เริ่มต้น] เปิดใช้ทุกฟีเจอร์");
    } else {
        eprintln!(
            "[คำเตือน] ฟีเจอร์ที่ปิดอยู่ (เปิดได้ที่ Developer Portal > Bot > Privileged Gateway Intents แล้วรีสตาร์ทบอท):"
        );
        for item in off {
            eprintln!("  - {item}");
        }
    }
}

/// สร้างและรันบอทหนึ่งรอบ คืน error ดิบเพื่อให้ตัวเรียกตัดสินใจว่าจะลองใหม่ไหม
async fn start_client(
    token: &str,
    features: Features,
    config: Arc<Config>,
) -> Result<(), Box<serenity::Error>> {
    let framework = build_framework(features, config);
    let mut client = serenity::ClientBuilder::new(token, features.intents())
        .framework(framework)
        .register_songbird()
        .await
        .map_err(Box::new)?;

    // กด Ctrl+C เพื่อปิดบอทอย่างนุ่มนวล
    let shard_manager = client.shard_manager.clone();
    let ctrl_c = tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                println!("\n[ปิดบอท] กำลังปิดบอท...");
                shard_manager.shutdown_all().await;
            }
            Err(e) => eprintln!("[คำเตือน] ดักปุ่ม Ctrl+C ไม่ได้: {e}"),
        }
    });

    let result = client.start().await;
    ctrl_c.abort();
    result.map_err(Box::new)
}

fn build_framework(features: Features, config: Arc<Config>) -> poise::Framework<Data, Error> {
    poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some(config.prefix.clone()),
                mention_as_prefix: true,
                case_insensitive_commands: true,
                ..Default::default()
            },
            on_error: |err| Box::pin(error::on_error(err)),
            pre_command: |ctx| {
                Box::pin(async move {
                    println!(
                        "[คำสั่ง] {} ใช้คำสั่ง {}",
                        ctx.author().name,
                        ctx.command().qualified_name
                    );
                })
            },
            event_handler: |ctx, event, framework, data| {
                Box::pin(events::handle(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                // ลงทะเบียน slash command ทุกเซิร์ฟเวอร์ ถ้าไม่สำเร็จบอทยังใช้คำสั่งแบบ prefix ได้
                let commands = &framework.options().commands;
                match poise::builtins::register_globally(ctx, commands).await {
                    Ok(()) => println!("[เริ่มต้น] ลงทะเบียน slash command แล้ว"),
                    Err(e) => eprintln!("[คำเตือน] ลงทะเบียน slash command ไม่สำเร็จ: {e}"),
                }

                ctx.set_activity(Some(serenity::ActivityData::listening("/help | /play")));
                println!(
                    "[เริ่มต้น] {} ออนไลน์แล้ว! อยู่ใน {} เซิร์ฟเวอร์",
                    ready.user.name,
                    ready.guilds.len()
                );
                report_features(features);

                // อัปเดตห้องสถิติเป็นระยะ และเปิดหน้าเว็บจัดการ
                tokio::spawn(stats::run_loop(
                    ctx.clone(),
                    config.settings.clone(),
                    features,
                ));
                web::spawn(
                    ctx.clone(),
                    config.settings.clone(),
                    features,
                    config.music.clone(),
                );

                Ok(Data {
                    start_time: config.start_time,
                    prefix: config.prefix.clone(),
                    features,
                    music: config.music.clone(),
                    settings: config.settings.clone(),
                })
            })
        })
        .build()
}

/// แปล error ตอนเชื่อมต่อ Discord ให้อ่านเข้าใจ
fn explain_client_error(e: &serenity::Error) -> String {
    use serenity::GatewayError as G;
    match e {
        serenity::Error::Gateway(G::DisallowedGatewayIntents) => {
            "บอทขอ intent ที่ยังไม่ได้เปิด ไปที่ Discord Developer Portal > Bot > Privileged Gateway Intents"
                .to_string()
        }
        serenity::Error::Gateway(G::InvalidAuthentication) => {
            "token ไม่ถูกต้องหรือถูกรีเซ็ตไปแล้ว ไปสร้าง token ใหม่ใน Developer Portal แล้วใส่ใน .env".to_string()
        }
        serenity::Error::Gateway(G::InvalidGatewayIntents) => "ค่า intents ไม่ถูกต้อง".to_string(),
        other => format!("เชื่อมต่อ Discord ไม่สำเร็จ: {other}"),
    }
}
