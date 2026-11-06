//! VCD export of a simulation's waveforms (for GTKWave, Surfer, ...).

use std::fmt::Write;

use crate::SimResult;

fn id_code(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.push((b'!' + (n % 94) as u8) as char);
        n /= 94;
        if n == 0 {
            return s;
        }
    }
}

fn timescale(fs: u64) -> String {
    for (unit, size) in [
        ("s", 1_000_000_000_000_000u64),
        ("ms", 1_000_000_000_000),
        ("us", 1_000_000_000),
        ("ns", 1_000_000),
        ("ps", 1_000),
        ("fs", 1),
    ] {
        if fs >= size && fs.is_multiple_of(size) {
            return format!("{}{unit}", fs / size);
        }
    }
    "1ns".into()
}

pub fn to_vcd(r: &SimResult) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "$date Wafersmith simulation $end\n$version wafer-sim $end\n$timescale {} $end",
        timescale(r.timescale_fs)
    );
    // Build the scope tree from dotted names.
    let mut sigs: Vec<(Vec<&str>, usize)> =
        r.signals.iter().enumerate().map(|(i, s)| (s.name.split('.').collect(), i)).collect();
    sigs.sort();
    let mut open: Vec<&str> = Vec::new();
    for (path, i) in &sigs {
        let scopes = &path[..path.len() - 1];
        let common = open.iter().zip(scopes.iter()).take_while(|(a, b)| a == b).count();
        while open.len() > common {
            open.pop();
            out.push_str("$upscope $end\n");
        }
        for s in &scopes[common..] {
            let _ = writeln!(out, "$scope module {s} $end");
            open.push(s);
        }
        let sig = &r.signals[*i];
        let name = path.last().unwrap();
        let range = if sig.width > 1 { format!(" [{}:0]", sig.width - 1) } else { String::new() };
        let _ = writeln!(out, "$var wire {} {} {name}{range} $end", sig.width, id_code(*i));
    }
    for _ in open {
        out.push_str("$upscope $end\n");
    }
    out.push_str("$enddefinitions $end\n");

    // Merge all changes by time.
    let mut events: Vec<(u64, usize, u64)> = r
        .signals
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.times.iter().zip(&s.values).map(move |(&t, &v)| (t, i, v)))
        .collect();
    events.sort_by_key(|e| (e.0, e.1));
    let mut last_t = None;
    for (t, i, v) in events {
        if last_t != Some(t) {
            let _ = writeln!(out, "#{t}");
            last_t = Some(t);
        }
        let s = &r.signals[i];
        if s.width == 1 {
            let _ = writeln!(out, "{}{}", v & 1, id_code(i));
        } else {
            let _ = writeln!(out, "b{:b} {}", v, id_code(i));
        }
    }
    let _ = writeln!(out, "#{}", r.time);
    out
}
