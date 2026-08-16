use std::env;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn event(sequence: usize) -> String {
    format!(
        "{{\"nativeEvent\":\"done\",\"taskId\":\"task-{sequence}\",\"sourceEventId\":\"event-{sequence}\",\"occurredAt\":{}}}",
        now_millis()
    )
}

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "idle".into());
    if mode == "hold-pipes" {
        thread::sleep(Duration::from_secs(60));
        return;
    }

    let mut handshake = String::new();
    io::BufReader::new(io::stdin())
        .read_line(&mut handshake)
        .expect("read handshake");
    assert!(handshake.contains("\"protocolVersion\":1"));
    assert!(handshake.contains("\"profileId\":"));
    println!("{{\"type\":\"ready\",\"protocolVersion\":1}}");
    io::stdout().flush().unwrap();

    match mode.as_str() {
        "event-idle" => {
            println!("{}", event(1));
            io::stdout().flush().unwrap();
        }
        "overlong" => {
            io::stdout().write_all(&vec![b'x'; 16 * 1024 + 1]).unwrap();
            io::stdout().flush().unwrap();
        }
        "large-stdout" => {
            let mut stdout = io::BufWriter::new(io::stdout().lock());
            let padding = " ".repeat(15_000);
            for sequence in 0..80 {
                writeln!(stdout, "{}{padding}", event(sequence)).unwrap();
            }
            stdout.flush().unwrap();
        }
        "large-stderr" => {
            let chunk = [b'e'; 8 * 1024];
            let mut stderr = io::stderr().lock();
            for _ in 0..256 {
                stderr.write_all(&chunk).unwrap();
            }
            stderr.flush().unwrap();
            println!("{}", event(1));
            io::stdout().flush().unwrap();
        }
        "spawn-descendant" => {
            let child = Command::new(env::current_exe().unwrap())
                .arg("hold-pipes")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn descendant");
            std::mem::forget(child);
        }
        "exit" => {
            thread::sleep(Duration::from_millis(250));
            return;
        }
        "idle" => {}
        other => panic!("unknown fixture mode: {other}"),
    }
    thread::sleep(Duration::from_secs(60));
}
