# syntax=docker/dockerfile:1

# ---------- ขั้นที่ 1: คอมไพล์บอท ----------
FROM rust:1-trixie AS builder
# libopus_sys ต้องใช้ cmake ตอนคอมไพล์
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# cache ไว้ ครั้งต่อไปจะคอมไพล์เฉพาะส่วนที่แก้
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/discord_bot /usr/local/bin/discord_bot

# ---------- ขั้นที่ 2: โหลด yt-dlp (ตัวเดี่ยว ไม่ต้องลง Python) ----------
FROM debian:trixie-slim AS ytdlp
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN case "$TARGETARCH" in \
        amd64) file=yt-dlp_linux ;; \
        arm64) file=yt-dlp_linux_aarch64 ;; \
        *) echo "ไม่รองรับสถาปัตยกรรม $TARGETARCH" && exit 1 ;; \
    esac \
    && curl -fsSL -o /yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/latest/download/$file" \
    && chmod 755 /yt-dlp

# ---------- ขั้นที่ 3: ตัวที่ใช้รันจริง ----------
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bot \
    && mkdir -p /app/data \
    && chown bot:bot /app/data

# Deno = JavaScript runtime ที่ yt-dlp ใช้ถอดรหัสลิงก์เสียงของ YouTube
COPY --from=denoland/deno:bin /deno /usr/local/bin/deno
# ให้ผู้ใช้ bot เป็นเจ้าของ yt-dlp เพื่อให้อัปเดตตัวเองได้
COPY --from=ytdlp --chown=bot:bot /yt-dlp /usr/local/bin/yt-dlp
COPY --from=builder /usr/local/bin/discord_bot /usr/local/bin/discord_bot

USER bot
WORKDIR /app
# บอทปิดตัวอย่างนุ่มนวลเมื่อได้ Ctrl+C (SIGINT)
STOPSIGNAL SIGINT
# อัปเดต yt-dlp ทุกครั้งที่เริ่ม เพราะ YouTube เปลี่ยนระบบบ่อย เวอร์ชันเก่าจะเปิดเพลงไม่ได้
CMD ["sh", "-c", "yt-dlp -U || echo '[คำเตือน] อัปเดต yt-dlp ไม่สำเร็จ ใช้เวอร์ชันเดิม'; exec discord_bot"]
