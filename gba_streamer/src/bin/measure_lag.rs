use anyhow::Result;
use gba_streamer::core::RetroCore;
use std::env;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!("Usage: measure_lag <core_path> <rom_path>");
        return Ok(());
    }
    let core_path = &args[1];
    let rom_path = &args[2];

    let mut core = RetroCore::load(core_path, rom_path)?;
    core.set_runahead_frames(0);

    for _ in 0..60 {
        core.step();
    }

    core.set_input(0);
    let (_, baseline_frame, _) = core.step();

    let mut detected_lag = 1u8;
    for frame_idx in 0..6 {
        core.set_input((1 << 4) | (1 << 0));
        let (_, current_frame, _) = core.step();
        
        let diff_count = baseline_frame
            .iter()
            .zip(current_frame.iter())
            .filter(|(a, b)| a != b)
            .count();

        if diff_count > 50 {
            detected_lag = frame_idx as u8;
            break;
        }
    }

    println!("PROBE_RESULT: title='{}', code='{}', measured_lag={}", 
        core.rom_title, core.rom_game_code, detected_lag);

    Ok(())
}
