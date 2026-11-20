mod arena;
mod combat;
mod policy;
mod routing;

use policy::Bot;
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
    thread,
};

pub type Point = [f64; 2];
pub type Position = [f64; 3];

pub fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or_default()
}

pub fn items(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

pub fn xy(value: &Value) -> Point {
    [number(&value[0]), number(&value[1])]
}

pub fn xyz(value: &Value) -> Position {
    [number(&value[0]), number(&value[1]), number(&value[2])]
}

pub fn flat([x, y, _]: Position) -> Point {
    [x, y]
}

pub fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub fn bearing(a: Point, b: Point) -> f64 {
    (b[1] - a[1]).atan2(b[0] - a[0])
}

pub fn unit(yaw: f64) -> Point {
    [yaw.cos(), yaw.sin()]
}

pub fn angle_delta(a: f64, b: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    (a - b + PI).rem_euclid(TAU) - PI
}

pub fn offset(p: Point, direction: Point, distance: f64) -> Point {
    [
        p[0] + direction[0] * distance,
        p[1] + direction[1] * distance,
    ]
}

fn send(out: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *out, message)?;
    out.write_all(b"\n")?;
    out.flush()
}

fn main() -> io::Result<()> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut bot = None;
    while let Ok(first) = receiver.recv() {
        let mut latest = None;
        // Drain queued observations before deciding, as required by mechfight/1.
        for line in std::iter::once(first).chain(receiver.try_iter()) {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match message["type"].as_str() {
                Some("hello") if message["protocol"] == "mechfight/1" => {
                    bot = Some(Bot::new(message).map_err(io::Error::other)?);
                    send(&mut out, &json!({"type":"ready","name":"Oval Runner Rust"}))?;
                }
                Some("obs") => latest = Some(message),
                Some("end") => return Ok(()),
                _ => {}
            }
        }
        if let (Some(bot), Some(obs)) = (&mut bot, latest) {
            send(&mut out, &bot.act(&obs))?;
        }
    }
    Ok(())
}
