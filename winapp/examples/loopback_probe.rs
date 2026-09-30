//! SPIKE (throwaway): can stepwave capture one process's audio with WASAPI process loopback
//! while the user does not hear the original?
//!
//! Usage: loopback_probe [process.exe] [--play] [--seconds N]
//! Prints the captured level every 0.5 s. With --play, also renders the captured audio to the
//! default output device, so you can hear whether the capture still works after muting the
//! app in the Volume mixer or moving it to another output device.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(e) = win::run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
    const CH: usize = 2;

    fn format() -> WaveFormat {
        WaveFormat::new(32, 32, &SampleType::Float, 48000, CH, None)
    }

    fn find_pid(name: &str) -> Res<u32> {
        let out = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {name}")])
            .output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut pids: Vec<u32> = text
            .lines()
            .filter_map(|l| l.split("\",\"").nth(1)?.parse().ok())
            .collect();
        pids.sort();
        pids.first()
            .copied()
            .ok_or_else(|| format!("{name} is not running").into())
    }

    struct Stats {
        sum_sq: AtomicU32, // f32 bits
        peak: AtomicU32,
        frames: AtomicU32,
        packets: AtomicU32,
        silent_packets: AtomicU32,
    }

    fn add(a: &AtomicU32, v: f32) {
        let _ = a.fetch_update(Relaxed, Relaxed, |b| {
            Some((f32::from_bits(b) + v).to_bits())
        });
    }

    pub fn run() -> Res<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let play = args.iter().any(|a| a == "--play");
        let seconds: u64 = args
            .iter()
            .position(|a| a == "--seconds")
            .and_then(|i| args.get(i + 1)?.parse().ok())
            .unwrap_or(120);
        let name = args
            .iter()
            .find(|a| a.to_lowercase().ends_with(".exe"))
            .cloned()
            .unwrap_or_else(|| "cs2.exe".into());
        let pid = find_pid(&name)?;
        println!("capturing {name} (pid {pid}) for {seconds} s, play={play}");
        println!("try: mute {name} in the Volume mixer, or move it to another output device");

        let stats = Arc::new(Stats {
            sum_sq: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            frames: AtomicU32::new(0),
            packets: AtomicU32::new(0),
            silent_packets: AtomicU32::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(48000 * CH);

        let cap_stats = stats.clone();
        let cap_stop = stop.clone();
        let cap = std::thread::spawn(move || -> Res<()> {
            wasapi::initialize_mta().ok()?;
            // wasapi 0.24: `true` = PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE.
            let mut client = AudioClient::new_application_loopback_client(pid, true)?;
            let mode = StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 200_000,
            };
            client.initialize_client(&format(), &Direction::Capture, &mode)?;
            let event = client.set_get_eventhandle()?;
            let capture = client.get_audiocaptureclient()?;
            let mut bytes = vec![0u8; 48000 * CH * 4];
            client.start_stream()?;
            while !cap_stop.load(Relaxed) {
                if event.wait_for_event(500).is_err() {
                    continue; // no audio from the process: no packets at all
                }
                loop {
                    let n = capture.get_next_packet_size()?.unwrap_or(0) as usize;
                    if n == 0 {
                        break;
                    }
                    let n = n.min(48000);
                    let (read, info) = capture.read_from_device(&mut bytes[..n * CH * 4])?;
                    let read = read as usize;
                    cap_stats.packets.fetch_add(1, Relaxed);
                    if info.flags.silent {
                        cap_stats.silent_packets.fetch_add(1, Relaxed);
                    }
                    let mut ss = 0.0f32;
                    let mut pk = 0.0f32;
                    for b in bytes[..read * CH * 4].as_chunks::<4>().0 {
                        let s = if info.flags.silent {
                            0.0
                        } else {
                            f32::from_le_bytes(*b)
                        };
                        ss += s * s;
                        pk = pk.max(s.abs());
                        let _ = tx.push(s);
                    }
                    add(&cap_stats.sum_sq, ss);
                    let _ = cap_stats.peak.fetch_update(Relaxed, Relaxed, |b| {
                        Some(f32::from_bits(b).max(pk).to_bits())
                    });
                    cap_stats.frames.fetch_add(read as u32, Relaxed);
                }
            }
            client.stop_stream()?;
            Ok(())
        });

        let ren = if play {
            let ren_stop = stop.clone();
            let mut rx = rx;
            Some(std::thread::spawn(move || -> Res<()> {
                wasapi::initialize_mta().ok()?;
                let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
                println!("playing to: {}", device.get_friendlyname()?);
                let mut client = device.get_iaudioclient()?;
                let (period, _) = client.get_device_period()?;
                let mode = StreamMode::EventsShared {
                    autoconvert: true,
                    buffer_duration_hns: period,
                };
                client.initialize_client(&format(), &Direction::Render, &mode)?;
                let event = client.set_get_eventhandle()?;
                let render = client.get_audiorenderclient()?;
                let frames = client.get_buffer_size()? as usize;
                let mut bytes = vec![0u8; frames * CH * 4];
                client.start_stream()?;
                while !ren_stop.load(Relaxed) {
                    if event.wait_for_event(1000).is_err() {
                        return Err("render stalled".into());
                    }
                    // Keep latency bounded: drop backlog beyond ~50 ms.
                    while rx.slots() > 2400 * CH {
                        let _ = rx.pop();
                    }
                    let space = (client.get_available_space_in_frames()? as usize).min(frames);
                    for b in bytes[..space * CH * 4].as_chunks_mut::<4>().0 {
                        *b = rx.pop().unwrap_or(0.0).to_le_bytes();
                    }
                    render.write_to_device(space, &bytes[..space * CH * 4], None)?;
                }
                client.stop_stream()?;
                Ok(())
            }))
        } else {
            None
        };

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(seconds) {
            std::thread::sleep(Duration::from_millis(500));
            let frames = stats.frames.swap(0, Relaxed);
            let ss = f32::from_bits(stats.sum_sq.swap(0, Relaxed));
            let pk = f32::from_bits(stats.peak.swap(0, Relaxed));
            let packets = stats.packets.swap(0, Relaxed);
            let silent = stats.silent_packets.swap(0, Relaxed);
            let rms_db = if frames > 0 {
                10.0 * (ss / (frames as f32 * CH as f32) + 1e-12).log10()
            } else {
                f32::NEG_INFINITY
            };
            println!(
                "{:6.1}s  frames {frames:6}  packets {packets:4} (silent {silent:4})  rms {rms_db:7.1} dBFS  peak {:7.1} dBFS",
                start.elapsed().as_secs_f32(),
                20.0 * (pk + 1e-12).log10()
            );
            if cap.is_finished() {
                break;
            }
        }
        stop.store(true, Relaxed);
        cap.join().map_err(|_| "capture thread panicked")??;
        if let Some(r) = ren {
            r.join().map_err(|_| "render thread panicked")??;
        }
        Ok(())
    }
}
