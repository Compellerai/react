//! GigBAR Move — Open-DMX USB bench test.
//!
//! Fires EVERY section of a Chauvet DJ GigBAR Move (2 pars, 2 derbies, laser,
//! white/UV strobe, 2 moving heads) over an FTDI "Open DMX USB" cable so the
//! wiring + DMX addressing can be verified against the real fixture. Values come
//! straight from the GigBAR MOVE User Manual Rev. 4 DMX chart (35-Channel and
//! 17-Channel personalities).
//!
//! This is intentionally standalone: it uses the SAME `open_dmx` crate REACT's
//! `src/dmx/usb.rs` uses, but does NOT go through the app, the show
//! brain, the music autopilot, or the SaaS stage sync. In the live app the Laser
//! and Strobe lanes are gated dark at idle (they only fire on a drop / big
//! groove), so a passive test of the app would look like those sections "aren't
//! firing". This tool drives deterministic values so you can confirm the hardware
//! itself responds on every channel.
//!
//! USAGE
//!   cargo run -- [--port <dev>] [--personality 35|17] [--base <addr>]
//!               [--mode cycle|all|blackout|<section>] [--hold <secs>]
//!
//!   --port         FTDI cable device. Default: autodetect first /dev/cu.usbserial*.
//!   --personality  DMX mode the BAR's menu is set to (35 or 17 channel). Default 35.
//!   --base         The bar's DMX start address (its menu "d 001" etc.). Default 1.
//!   --mode         cycle   = light each section in turn (verify per-section
//!                            addressing), then ALL together, looping. [default]
//!                  all     = light every section at once (movers sweep).
//!                  blackout= send zeros (turn the bar off).
//!                  <name>  = light just one section: par1 par2 derby1 derby2
//!                            laser strobe mh1 mh2 (35ch) / pars laser strobe
//!                            heads (17ch).
//!   --hold         Seconds each section is held in cycle mode. Default 2.5.
//!
//! SAFETY: this is a LASER fixture. Point the bar at a wall, never at eyes, and
//! keep the strobe section in mind for photosensitivity. Ctrl-C blacks the bar
//! out cleanly before exit.

use std::time::{Duration, Instant};

use open_dmx::DMXSerial;

const DMX_SLOTS: usize = 512;

/// A 512-slot DMX universe buffer. `frame[0]` == DMX channel 1 (no start code),
/// matching REACT's convention and what `DMXSerial::set_channels` expects.
type Frame = [u8; DMX_SLOTS];

#[derive(Clone, Copy, PartialEq)]
enum Personality {
    Ch35,
    Ch17,
}

enum Mode {
    Cycle,
    All,
    Blackout,
    Section(String),
}

struct Config {
    port: Option<String>,
    personality: Personality,
    base: u16,
    mode: Mode,
    hold: f64,
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}\n");
            eprintln!("{}", USAGE);
            std::process::exit(2);
        }
    };

    let port = match cfg.port.clone().or_else(autodetect_port) {
        Some(p) => p,
        None => {
            eprintln!(
                "error: no --port given and no /dev/cu.usbserial* (macOS) or \
                 /dev/ttyUSB* (Linux) FTDI cable found.\nPlug in the Open DMX USB \
                 cable, or pass --port /dev/cu.usbserial-XXXX."
            );
            std::process::exit(1);
        }
    };

    let sections = section_table(cfg.personality);
    let section_names: Vec<&str> = sections.iter().map(|s| s.name).collect();

    // Validate a named section up front so we fail before opening the port.
    if let Mode::Section(name) = &cfg.mode {
        if !section_names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            eprintln!(
                "error: unknown section '{name}' for {} personality.\nvalid: {}",
                personality_label(cfg.personality),
                section_names.join(" ")
            );
            std::process::exit(2);
        }
    }

    println!("GigBAR Move — Open DMX USB bench test");
    println!("  port         {port}");
    println!("  personality  {}", personality_label(cfg.personality));
    println!("  base address {}", cfg.base);
    println!("  channels     {}-{}", cfg.base, cfg.base + channel_span(cfg.personality) - 1);
    println!("  mode         {}", mode_label(&cfg.mode));
    println!("  sections     {}", section_names.join(" "));
    println!();
    println!("⚠️  LASER FIXTURE — aim at a wall, never at eyes. Strobe section can flash.");
    println!("   Set the bar's menu to {} personality, start address {}.", personality_label(cfg.personality), cfg.base);
    println!("   Press Ctrl-C to black out and exit.\n");

    let mut dmx = match DMXSerial::open(&port) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot open '{port}': {e}");
            eprintln!("Is another process (REACT, a visualizer) holding the cable? Close it first.");
            std::process::exit(1);
        }
    };

    // Clean blackout on Ctrl-C: flip the flag, the main loop sends one zero frame
    // and exits. open_dmx keeps retransmitting in the background, so without this
    // the bar would hold its last look after we quit.
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let r = running.clone();
        if let Err(e) = ctrlc::set_handler(move || {
            r.store(false, std::sync::atomic::Ordering::SeqCst);
        }) {
            eprintln!("warning: could not install Ctrl-C handler ({e}); the bar may hold its last look on exit.");
        }
    }

    let start = Instant::now();
    let tick = Duration::from_millis(30); // ~33 fps refresh
    let mut last_active: Option<usize> = None;

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        let t = start.elapsed().as_secs_f64();
        let mut frame: Frame = [0u8; DMX_SLOTS];

        match &cfg.mode {
            Mode::Blackout => { /* leave zeros */ }
            Mode::All => {
                for s in &sections {
                    (s.fill)(&mut frame, cfg.base, t);
                }
            }
            Mode::Section(name) => {
                if let Some(s) = sections.iter().find(|s| s.name.eq_ignore_ascii_case(name)) {
                    (s.fill)(&mut frame, cfg.base, t);
                }
            }
            Mode::Cycle => {
                // sections.len() single-section steps, then one "ALL" step.
                let steps = sections.len() + 1;
                let idx = ((t / cfg.hold) as usize) % steps;
                if idx < sections.len() {
                    (sections[idx].fill)(&mut frame, cfg.base, t);
                } else {
                    for s in &sections {
                        (s.fill)(&mut frame, cfg.base, t);
                    }
                }
                if last_active != Some(idx) {
                    let label = if idx < sections.len() { sections[idx].name } else { "ALL sections" };
                    println!("  ▶ {label}");
                    last_active = Some(idx);
                }
            }
        }

        dmx.set_channels(frame);
        std::thread::sleep(tick);
    }

    // Graceful blackout: zero the universe and give the background thread a beat
    // to push it down the cable before the process (and its retransmit) dies.
    dmx.set_channels([0u8; DMX_SLOTS]);
    std::thread::sleep(Duration::from_millis(80));
    println!("\nblacked out — bye.");
}

// ---------------------------------------------------------------------------
// Section fill functions. `ch` is the 1-based channel WITHIN the fixture
// (1..=35 or 1..=17); `set` maps it to the absolute DMX slot at `base`.
// Values are taken from the GigBAR MOVE User Manual Rev. 4 DMX chart.
// ---------------------------------------------------------------------------

struct Section {
    name: &'static str,
    fill: fn(&mut Frame, base: u16, t: f64),
}

/// Write channel `ch` (1-based, within the fixture) at start address `base`.
#[inline]
fn set(frame: &mut Frame, base: u16, ch: u16, value: u8) {
    // absolute DMX channel = base + (ch - 1); slot index = absolute - 1.
    let abs = base as u32 + ch as u32 - 1;
    if (1..=DMX_SLOTS as u32).contains(&abs) {
        frame[(abs - 1) as usize] = value;
    }
}

/// Slow triangle/sine sweep value centered at 128 for visible mover motion.
#[inline]
fn sweep(t: f64, period: f64, amplitude: f64, phase: f64) -> u8 {
    let v = 128.0 + amplitude * (2.0 * std::f64::consts::PI * (t / period) + phase).sin();
    v.clamp(0.0, 255.0) as u8
}

fn section_table(p: Personality) -> Vec<Section> {
    match p {
        Personality::Ch35 => vec![
            Section { name: "par1",   fill: fill_par1_35 },
            Section { name: "par2",   fill: fill_par2_35 },
            Section { name: "derby1", fill: fill_derby1_35 },
            Section { name: "derby2", fill: fill_derby2_35 },
            Section { name: "laser",  fill: fill_laser_35 },
            Section { name: "strobe", fill: fill_strobe_35 },
            Section { name: "mh1",    fill: fill_mh1_35 },
            Section { name: "mh2",    fill: fill_mh2_35 },
        ],
        Personality::Ch17 => vec![
            Section { name: "pars",   fill: fill_pars_17 },
            Section { name: "laser",  fill: fill_laser_17 },
            Section { name: "strobe", fill: fill_strobe_17 },
            Section { name: "heads",  fill: fill_heads_17 },
        ],
    }
}

// ---- 35-Channel personality ------------------------------------------------
// Ch1-4 Par1 R/G/B/UV, Ch5 Par1 dim/strobe (0-127 = RGB dim 0-100%, 127=full).
fn fill_par1_35(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 1, 255); // red
    set(f, b, 2, 255); // green
    set(f, b, 3, 255); // blue  -> white
    set(f, b, 4, 0);   // UV
    set(f, b, 5, 127); // dimmer full, no strobe (0-127 zone)
}
// Ch6-9 Par2 R/G/B/UV, Ch10 Par2 dim/strobe. Tinted so it's distinguishable.
fn fill_par2_35(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 6, 0);   // red
    set(f, b, 7, 180); // green
    set(f, b, 8, 255); // blue  -> cyan
    set(f, b, 9, 0);   // UV
    set(f, b, 10, 127);
}
// Ch11 Derby1 color (175 = R+G+B), Ch12 strobe (0 = none), Ch13 rotation (CW).
fn fill_derby1_35(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 11, 175); // all three colors
    set(f, b, 12, 0);   // no strobe -> steady
    set(f, b, 13, 70);  // rotate clockwise, slow-mid
}
// Ch14-16 Derby2 (counter-rotate via CCW zone 134-255).
fn fill_derby2_35(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 14, 175);
    set(f, b, 15, 0);
    set(f, b, 16, 190); // rotate counter-clockwise
}
// Ch17 Laser color (120 = red + green on), Ch18 strobe (0), Ch19 pattern (CW).
fn fill_laser_35(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 17, 120); // red + green lasers on
    set(f, b, 18, 0);   // no strobe
    set(f, b, 19, 70);  // pattern rotate CW
}
// Ch20 Strobe Control (0 = no auto program), Ch21 White dimmer (255 = solid white),
// Ch22 UV (0 — 21 & 22 are mutually exclusive), Ch23 Speed (n/a when solid).
fn fill_strobe_35(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 20, 0);   // no auto-strobe program
    set(f, b, 21, 255); // white LEDs solid on
    set(f, b, 22, 0);   // UV off
    set(f, b, 23, 0);   // speed unused while solid
}
// Ch24-29 Moving Head 1: pan/tilt sweep, full dimmer, open white, open gobo.
fn fill_mh1_35(f: &mut Frame, b: u16, t: f64) {
    set(f, b, 24, sweep(t, 6.0, 45.0, 0.0));  // pan
    set(f, b, 25, sweep(t, 4.0, 30.0, 0.0));  // tilt
    set(f, b, 26, 255);                       // dimmer
    set(f, b, 27, 0);                         // strobe / shutter open
    set(f, b, 28, 16);                        // color wheel slot 1 (open/white)
    set(f, b, 29, 16);                        // gobo slot 1 (open)
}
// Ch30-35 Moving Head 2: counter-phase sweep so the two heads visibly differ.
fn fill_mh2_35(f: &mut Frame, b: u16, t: f64) {
    set(f, b, 30, sweep(t, 6.0, 45.0, std::f64::consts::PI)); // pan, opposite phase
    set(f, b, 31, sweep(t, 4.0, 30.0, std::f64::consts::PI)); // tilt, opposite phase
    set(f, b, 32, 255);
    set(f, b, 33, 0);
    set(f, b, 34, 16);
    set(f, b, 35, 16);
}

// ---- 17-Channel personality ------------------------------------------------
// Ch1-4 Pars+Derby color R/G/B/UV, Ch5 dim/strobe (0-127 dimmer, 127=full).
fn fill_pars_17(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 1, 255);
    set(f, b, 2, 255);
    set(f, b, 3, 255);
    set(f, b, 4, 0);
    set(f, b, 5, 127); // full dimmer, no strobe
    set(f, b, 6, 70);  // derby rotation CW
}
// Ch7 Red laser (5-54 on), Ch8 Green laser (5-54 on), Ch9 movement (CW).
fn fill_laser_17(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 7, 30);  // red laser on
    set(f, b, 8, 30);  // green laser on
    set(f, b, 9, 70);  // movement rotate CW
}
// Ch10 White strobe (0-54 = white 0-100%, 54 = solid), Ch11 UV (0; mutually excl).
fn fill_strobe_17(f: &mut Frame, b: u16, _t: f64) {
    set(f, b, 10, 54); // white LEDs solid (top of the 0-100% zone)
    set(f, b, 11, 0);  // UV off
}
// Ch12-17 Moving heads (one control drives both): pan/tilt sweep, full, open.
fn fill_heads_17(f: &mut Frame, b: u16, t: f64) {
    set(f, b, 12, sweep(t, 6.0, 45.0, 0.0)); // pan
    set(f, b, 13, sweep(t, 4.0, 30.0, 0.0)); // tilt
    set(f, b, 14, 255);                      // dimmer
    set(f, b, 15, 0);                        // strobe open
    set(f, b, 16, 16);                       // color open/white
    set(f, b, 17, 16);                       // gobo open
}

// ---------------------------------------------------------------------------
// Port autodetect (mirrors REACT's usb.rs::autodetect_port).
// ---------------------------------------------------------------------------

fn autodetect_port() -> Option<String> {
    let mut found: Vec<String> = std::fs::read_dir("/dev")
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("cu.usbserial") || name.starts_with("ttyUSB"))
        .map(|name| format!("/dev/{name}"))
        .collect();
    found.sort();
    found.into_iter().next()
}

// ---------------------------------------------------------------------------
// Argument parsing.
// ---------------------------------------------------------------------------

const USAGE: &str = "\
gigbar-usb-test — fire every section of a Chauvet GigBAR Move over Open DMX USB

USAGE:
  cargo run -- [--port <dev>] [--personality 35|17] [--base <addr>]
              [--mode cycle|all|blackout|<section>] [--hold <secs>]

OPTIONS:
  --port <dev>          FTDI cable, e.g. /dev/cu.usbserial-AB0MWWLR (default: autodetect)
  --personality 35|17   DMX mode set on the bar's menu (default: 35)
  --base <addr>         bar's DMX start address 1-512 (default: 1)
  --mode <m>            cycle (default) | all | blackout | <section name>
  --hold <secs>         seconds per section in cycle mode (default: 2.5)
  -h, --help            this help

SECTIONS:
  35ch: par1 par2 derby1 derby2 laser strobe mh1 mh2
  17ch: pars laser strobe heads";

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config {
        port: None,
        personality: Personality::Ch35,
        base: 1,
        mode: Mode::Cycle,
        hold: 2.5,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--port" => cfg.port = Some(args.next().ok_or("--port needs a value")?),
            "--personality" => {
                cfg.personality = match args.next().as_deref() {
                    Some("35") => Personality::Ch35,
                    Some("17") => Personality::Ch17,
                    other => return Err(format!("--personality must be 35 or 17 (got {other:?})")),
                };
            }
            "--base" => {
                let v = args.next().ok_or("--base needs a value")?;
                let n: u16 = v.parse().map_err(|_| format!("--base not a number: {v}"))?;
                if !(1..=512).contains(&n) {
                    return Err(format!("--base must be 1-512 (got {n})"));
                }
                cfg.base = n;
            }
            "--mode" => {
                let v = args.next().ok_or("--mode needs a value")?;
                cfg.mode = match v.to_ascii_lowercase().as_str() {
                    "cycle" => Mode::Cycle,
                    "all" => Mode::All,
                    "blackout" | "off" => Mode::Blackout,
                    _ => Mode::Section(v),
                };
            }
            "--hold" => {
                let v = args.next().ok_or("--hold needs a value")?;
                cfg.hold = v.parse().map_err(|_| format!("--hold not a number: {v}"))?;
                if cfg.hold <= 0.0 {
                    return Err("--hold must be > 0".into());
                }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(cfg)
}

fn personality_label(p: Personality) -> &'static str {
    match p {
        Personality::Ch35 => "35-channel",
        Personality::Ch17 => "17-channel",
    }
}

fn channel_span(p: Personality) -> u16 {
    match p {
        Personality::Ch35 => 35,
        Personality::Ch17 => 17,
    }
}

fn mode_label(m: &Mode) -> String {
    match m {
        Mode::Cycle => "cycle".into(),
        Mode::All => "all".into(),
        Mode::Blackout => "blackout".into(),
        Mode::Section(s) => format!("section:{s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_maps_to_absolute_channel() {
        let mut f: Frame = [0u8; DMX_SLOTS];
        // base 1, channel 1 -> slot 0.
        set(&mut f, 1, 1, 200);
        assert_eq!(f[0], 200);
        // base 10, channel 5 -> absolute 14 -> slot 13.
        set(&mut f, 10, 5, 99);
        assert_eq!(f[13], 99);
    }

    #[test]
    fn set_ignores_out_of_range() {
        let mut f: Frame = [0u8; DMX_SLOTS];
        // base 510, channel 35 -> absolute 544 -> dropped, no panic.
        set(&mut f, 510, 35, 255);
        assert!(f.iter().all(|&v| v == 0));
    }

    #[test]
    fn fire_all_35_lights_every_section_block() {
        // With all 8 sections filled at base 1, every section's lead channel is non-zero.
        let mut f: Frame = [0u8; DMX_SLOTS];
        for s in section_table(Personality::Ch35) {
            (s.fill)(&mut f, 1, 0.0);
        }
        // par1 r(ch1), par2 b(ch8), derby1(ch11), derby2(ch14), laser(ch17),
        // strobe white(ch21), mh1 dimmer(ch26), mh2 dimmer(ch32).
        for ch in [1u16, 8, 11, 14, 17, 21, 26, 32] {
            assert!(f[(ch - 1) as usize] > 0, "channel {ch} should be lit");
        }
    }

    #[test]
    fn span_matches_personality() {
        assert_eq!(channel_span(Personality::Ch35), 35);
        assert_eq!(channel_span(Personality::Ch17), 17);
    }
}
