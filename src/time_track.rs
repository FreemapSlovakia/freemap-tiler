use crate::tile::Tile;
use std::{
    fmt::{self, Display, Formatter},
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub enum StatsMsg {
    Duration(Metric, Duration),
    Stats(f32, usize, Tile),
}

pub enum Metric {
    Select,
    Insert,
    Encode,
    Warp,
    Compose,
}

#[derive(Default)]
struct TimeTrack {
    count: u32,
    duration: Duration,
}

impl TimeTrack {
    fn add(&mut self, duration: Duration) {
        self.duration += duration;
        self.count += 1;
    }
}

impl Display for TimeTrack {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            if self.count == 0 {
                "-".into()
            } else {
                format!(
                    "{}/{}={}",
                    self.duration.as_millis(),
                    self.count,
                    (self.duration / self.count).as_millis()
                )
            }
        )
    }
}

#[derive(Default)]
pub struct TimeStats {
    select: TimeTrack,
    insert: TimeTrack,
    warp: TimeTrack,
    compose: TimeTrack,
    encode: TimeTrack,
}

impl TimeStats {
    pub fn add(&mut self, metric: &Metric, duration: Duration) {
        match metric {
            Metric::Select => self.select.add(duration),
            Metric::Insert => self.insert.add(duration),
            Metric::Warp => self.warp.add(duration),
            Metric::Compose => self.compose.add(duration),
            Metric::Encode => self.encode.add(duration),
        }
    }
}

impl Display for TimeStats {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "select: {}, insert: {}, warp: {}, compose: {}, processing: {} | {}",
            self.select,
            self.insert,
            self.warp,
            self.compose,
            self.encode,
            (self.select.duration
                + self.insert.duration
                + self.warp.duration
                + self.compose.duration)
                .as_millis()
        )
    }
}

pub fn new(debug: bool) -> (Sender<StatsMsg>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<StatsMsg>();

    let mut stats = TimeStats::default();

    let mut last_log = Instant::now();

    let mut pct = 0_f32;

    let mut queue_len = 0_usize;

    let mut tile = Tile {
        x: 0,
        y: 0,
        zoom: 0,
    };

    let thread = thread::spawn(move || {
        for msg in rx {
            match msg {
                StatsMsg::Duration(typ, duration) => {
                    let now = Instant::now();

                    if now.duration_since(last_log).as_secs() > 10 {
                        last_log = now;

                        if debug {
                            print!("\n");
                        }

                        println!("{pct:.2} % | {queue_len} | {tile} | {stats}");

                        stats = TimeStats::default();
                    }

                    stats.add(&typ, duration);
                }
                StatsMsg::Stats(pct_, queue_len_, tile_) => {
                    pct = pct_;
                    queue_len = queue_len_;
                    tile = tile_;
                }
            }
        }
    });

    (tx, thread)
}
