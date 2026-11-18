//! Diagnostic : consommation du jour lue dans des transcriptions Claude Code.
//! `cargo run -p horizon-core --example claude_usage -- <session.jsonl> [autre.jsonl…]`

use horizon_core::claude_usage::{UsageBook, fresh_label};

fn main() {
    let mut book = UsageBook::default();
    for (i, p) in std::env::args().skip(1).enumerate() {
        book.track(&format!("s{i}"), "", p.into());
    }
    let now = horizon_core::time::now_ms();
    let t = std::time::Instant::now();
    book.refresh(now, &[]);
    let v = book.view(horizon_core::time::local_day_start_ms(now));
    println!("lu en {:?}", t.elapsed());
    for s in &v.sessions {
        println!("{} : {} ; contexte {:?} / {}", s.id, fresh_label(&s.today), s.context, s.window);
    }
    println!("total : {}", fresh_label(&v.today));
}
