//! Sonde du widget casque (lecture seule) : casque HID, sa sortie audio, à défaut casque
//! Bluetooth, et durée de chaque lecture.
//!
//! `cargo run -p horizon-sys --example headset_probe`

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut reader = horizon_sys::headset::Reader::new()?;
    for i in 1..=3 {
        let t = Instant::now();
        let r = reader.poll();
        println!("lecture {i} ({:.1} ms) : {r:?}", t.elapsed().as_secs_f64() * 1000.0);
    }
    for &(vid, pid) in reader.present() {
        let t = Instant::now();
        let outs = horizon_sys::mixer::outputs_of_usb(vid, pid);
        println!("sorties audio de {vid:04x}:{pid:04x} ({:.1} ms) : {outs:?}", t.elapsed().as_secs_f64() * 1000.0);
    }
    let t = Instant::now();
    let devices = horizon_sys::bluetooth_battery::devices();
    println!("énumération PnP Bluetooth ({:.1} ms) :", t.elapsed().as_secs_f64() * 1000.0);
    for d in &devices {
        println!("  {d:?} casque={}", d.is_headset());
    }
    let mut bt = horizon_sys::bluetooth_battery::Reader::default();
    for i in 1..=3 {
        let t = Instant::now();
        let r = bt.poll();
        println!("Bluetooth seul {i} ({:.1} ms) : {r:?}", t.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(())
}
