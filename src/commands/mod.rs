mod fun;
mod general;
mod moderation;
mod music;
mod server;

use crate::{Data, Error};

/// รวมคำสั่งทั้งหมด เพิ่มคำสั่งใหม่ได้ที่นี่
pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![
        // ทั่วไป
        general::help(),
        general::ping(),
        general::botinfo(),
        general::userinfo(),
        general::serverinfo(),
        general::avatar(),
        // เพลง
        music::play(),
        music::skip(),
        music::stop(),
        music::pause(),
        music::queue(),
        music::nowplaying(),
        music::volume(),
        music::loop_track(),
        music::shuffle(),
        music::remove(),
        music::leave(),
        // สมาชิก
        server::members(),
        // ตั้งค่า
        server::setup(),
        // สนุกๆ
        fun::roll(),
        fun::coinflip(),
        fun::eightball(),
        fun::choose(),
        fun::poll(),
        fun::say(),
        // ดูแลเซิร์ฟเวอร์
        moderation::clear(),
        moderation::kick(),
        moderation::ban(),
        moderation::unban(),
        moderation::timeout(),
        moderation::untimeout(),
    ]
}

/// สีหลักของ embed
pub const COLOR: u32 = 0x5865F2;

/// ตัดข้อความให้ยาวไม่เกิน `max` ตัวอักษร (Discord จำกัดความยาวหลายจุด)
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// กันอักขระ markdown ในข้อความจากผู้ใช้/YouTube ไม่ให้ทำรูปแบบเพี้ยน
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '*' | '_' | '~' | '`' | '|' | '[' | ']' | '\\' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("สวัสดี", 10), "สวัสดี");
        assert_eq!(truncate("abcdefghij", 6), "abc...");
        assert_eq!(truncate("กขคงจฉชซ", 5), "กข...");
    }

    #[test]
    fn escape_markdown() {
        assert_eq!(escape("a*b_c"), r"a\*b\_c");
        assert_eq!(escape("[title](x)"), r"\[title\](x)");
    }
}
