use anyhow::Result;
use lh_io::devices;

pub fn run() -> Result<()> {
    println!("Audio devices (host: {}):", devices::host_name());
    for dev in devices::enumerate()? {
        let mut tags = Vec::new();
        if dev.is_default_input {
            tags.push("default in");
        }
        if dev.is_default_output {
            tags.push("default out");
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", tags.join(", "))
        };
        println!("  [{}] {}{}", dev.index, dev.name, tags);
        for (label, port) in [("in ", &dev.input), ("out", &dev.output)] {
            if let Some(p) = port {
                let rates = if p.min_rate == p.max_rate {
                    format!("{} Hz", p.default_rate)
                } else {
                    format!("{} Hz ({}–{})", p.default_rate, p.min_rate, p.max_rate)
                };
                let buffer = p
                    .buffer_range
                    .map(|(lo, hi)| format!(", buffer {lo}–{hi}"))
                    .unwrap_or_default();
                println!(
                    "        {label}: {} ch {} @ {rates}{buffer}",
                    p.channels, p.sample_format
                );
            }
        }
    }
    Ok(())
}
