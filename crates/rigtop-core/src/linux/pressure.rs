//! Pressure stall information, readable without root: the share of time
//! tasks waited for CPU, memory or I/O, in `/proc/pressure/{cpu,memory,io}`
//! (Linux 4.20 and later):
//!
//! ```text
//! some avg10=5.61 avg60=5.53 avg300=6.31 total=16142608898
//! full avg10=5.54 avg60=5.34 avg300=6.09 total=15770090366
//! ```
//!
//! Kernels built without PSI have no such files; booted with `psi=0`, the
//! files exist but reading them fails. Either way there is nothing to show.

use std::fs;

use super::SysRoots;
use crate::snapshot::{Pressure, PressureLevels};

/// Reads the three resources, or `None` when PSI is not available.
pub fn read_pressure(roots: &SysRoots) -> Option<Pressure> {
    let read = |name: &str| {
        let text = fs::read_to_string(roots.proc.join("pressure").join(name)).ok()?;
        parse_levels(&text)
    };
    Some(Pressure {
        cpu: read("cpu")?,
        memory: read("memory")?,
        io: read("io")?,
    })
}

/// The `some` and `full` averages of one file. `full` is missing for the
/// CPU on kernels before 5.13 (zero then, as newer kernels report it).
fn parse_levels(text: &str) -> Option<PressureLevels> {
    let mut levels = PressureLevels::default();
    let mut found = false;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let kind = fields.next();
        let mut avg10 = None;
        let mut avg60 = None;
        for field in fields {
            match field.split_once('=') {
                Some(("avg10", value)) => avg10 = value.parse::<f32>().ok(),
                Some(("avg60", value)) => avg60 = value.parse::<f32>().ok(),
                _ => {}
            }
        }
        let (Some(avg10), Some(avg60)) = (avg10, avg60) else {
            continue;
        };
        match kind {
            Some("some") => {
                levels.some_avg10 = avg10;
                levels.some_avg60 = avg60;
                found = true;
            }
            Some("full") => {
                levels.full_avg10 = avg10;
                levels.full_avg60 = avg60;
            }
            _ => {}
        }
    }
    found.then_some(levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::write;

    #[test]
    fn reads_some_and_full_averages() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysRoots {
            sys: tmp.path().join("sys"),
            proc: tmp.path().join("proc"),
        };
        // No PSI at all.
        assert_eq!(read_pressure(&roots), None);

        // An older kernel: no "full" line for the CPU.
        write(
            &roots.proc,
            "pressure/cpu",
            "some avg10=12.50 avg60=8.00 avg300=3.10 total=171527842\n",
        );
        write(
            &roots.proc,
            "pressure/memory",
            "some avg10=0.00 avg60=0.40 avg300=0.00 total=7286113\n\
             full avg10=0.00 avg60=0.10 avg300=0.00 total=7140177\n",
        );
        // Without I/O, nothing (a resource missing means PSI is broken).
        assert_eq!(read_pressure(&roots), None);
        write(
            &roots.proc,
            "pressure/io",
            "some avg10=5.61 avg60=5.53 avg300=6.31 total=16142608898\n\
             full avg10=5.54 avg60=5.34 avg300=6.09 total=15770090366\n",
        );
        let pressure = read_pressure(&roots).unwrap();
        assert_eq!(pressure.cpu.some_avg10, 12.5);
        assert_eq!(pressure.cpu.full_avg60, 0.0);
        assert_eq!(pressure.memory.some_avg60, 0.4);
        assert_eq!(pressure.memory.full_avg60, 0.1);
        assert_eq!(pressure.io.full_avg10, 5.54);
        assert_eq!(pressure.io.full_avg60, 5.34);

        // psi=0: the files are there but empty to read.
        write(&roots.proc, "pressure/io", "");
        assert_eq!(read_pressure(&roots), None);
    }
}
