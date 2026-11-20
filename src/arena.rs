use crate::{flat, items, number, offset, Point, Position};
use serde_json::Value;
use std::{collections::HashMap, fs, io, path::Path};

const CELL: f64 = 2.0;

#[derive(Clone, Copy)]
pub struct Body {
    pub radius: f64,
    pub height: f64,
    pub step: f64,
    pub slope: f64,
}

impl Body {
    pub fn from_stats(stats: &Value) -> Self {
        Self {
            radius: number(&stats["radius"]),
            height: number(&stats["height"]),
            step: number(&stats["step_up"]),
            slope: number(&stats["max_slope_deg"]).to_radians().tan(),
        }
    }
}

pub struct Arena {
    bounds: [f64; 4],
    origin: Point,
    step: f64,
    scale: f64,
    width: usize,
    height: usize,
    terrain: Vec<i16>,
    solids: HashMap<(i32, i32), Vec<[f64; 2]>>,
}

impl Arena {
    pub fn load(path: &Path) -> io::Result<Self> {
        let path = path.join("v2");
        let world: Value = serde_json::from_slice(&fs::read(path.join("world.json"))?)?;
        let grid = &world["terrain"];
        let bounds = &world["bounds"];
        let mut arena = Self {
            bounds: ["minX", "maxX", "minY", "maxY"].map(|key| number(&bounds[key])),
            origin: [number(&grid["minX"]), number(&grid["minY"])],
            step: number(&grid["step"]),
            scale: number(&grid["scale"]),
            width: grid["nx"].as_u64().unwrap_or_default() as usize,
            height: grid["ny"].as_u64().unwrap_or_default() as usize,
            terrain: fs::read(path.join("terrain.i16"))?
                .as_chunks::<2>()
                .0
                .iter()
                .copied()
                .map(i16::from_le_bytes)
                .collect(),
            solids: HashMap::new(),
        };
        if arena.width < 2
            || arena.height < 2
            || arena.step <= 0.0
            || arena.terrain.len() != arena.width * arena.height
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid arena terrain",
            ));
        }
        for prism in items(&world["prisms"]) {
            arena.rasterize(prism);
        }
        Ok(arena)
    }

    fn rasterize(&mut self, prism: &Value) {
        let mut edges = Vec::new();
        for ring in std::iter::once(&prism["ring"]).chain(items(&prism["holes"])) {
            let points: Vec<Point> = items(ring)
                .as_chunks::<2>()
                .0
                .iter()
                .map(|point| [number(&point[0]), number(&point[1])])
                .collect();
            edges.extend(
                points
                    .iter()
                    .zip(points.iter().cycle().skip(1))
                    .map(|(&a, &b)| (a, b)),
            );
        }
        if edges.is_empty() {
            return;
        }
        let low = edges
            .iter()
            .map(|(a, _)| a[1])
            .fold(f64::INFINITY, f64::min);
        let high = edges
            .iter()
            .map(|(a, _)| a[1])
            .fold(f64::NEG_INFINITY, f64::max);
        let span = [number(&prism["base"]), number(&prism["top"])];
        let mut crossings = Vec::new();
        for row in (low / CELL).floor() as i32..(high / CELL).ceil() as i32 {
            let y = (f64::from(row) + 0.5) * CELL;
            crossings.clear();
            for &([x1, y1], [x2, y2]) in &edges {
                if (y1 > y) != (y2 > y) {
                    crossings.push(x1 + (x2 - x1) * (y - y1) / (y2 - y1));
                }
            }
            crossings.sort_unstable_by(f64::total_cmp);
            for pair in crossings.as_chunks::<2>().0 {
                let first = (pair[0] / CELL - 0.5).ceil() as i32;
                let last = (pair[1] / CELL - 0.5).ceil() as i32;
                for col in first..last {
                    self.solids.entry((col, row)).or_default().push(span);
                }
            }
        }
    }

    pub fn ground(&self, [x, y]: Point) -> f64 {
        let fx = ((x - self.origin[0]) / self.step).clamp(0.0, self.width as f64 - 1.00001);
        let fy = ((y - self.origin[1]) / self.step).clamp(0.0, self.height as f64 - 1.00001);
        let (col, row) = (fx as usize, fy as usize);
        let (dx, dy) = (fx.fract(), fy.fract());
        let i = row * self.width + col;
        let j = i + self.width;
        self.scale
            * ((1.0 - dy)
                * ((1.0 - dx) * f64::from(self.terrain[i]) + dx * f64::from(self.terrain[i + 1]))
                + dy * ((1.0 - dx) * f64::from(self.terrain[j])
                    + dx * f64::from(self.terrain[j + 1])))
    }

    pub fn spans(&self, [x, y]: Point) -> &[[f64; 2]] {
        self.solids
            .get(&((x / CELL).floor() as i32, (y / CELL).floor() as i32))
            .map_or(&[], Vec::as_slice)
    }

    pub fn within(&self, [x, y]: Point, radius: f64) -> bool {
        let [left, right, bottom, top] = self.bounds;
        x >= left + radius && x <= right - radius && y >= bottom + radius && y <= top - radius
    }

    pub fn travel(&self, pos: Position, direction: Point, distance: f64, body: Body) -> f64 {
        let mut feet = pos[2];
        let sideways = [-direction[1], direction[0]];
        for metre in 1..=distance.ceil() as u32 {
            let ahead = distance.min(f64::from(metre));
            let point = offset(flat(pos), direction, ahead);
            let floor = self.ground(point);
            if !self.within(point, body.radius)
                || floor > feet + body.step.max(body.slope)
                || [0.0, body.radius, -body.radius].iter().any(|&width| {
                    self.spans(offset(point, sideways, width))
                        .iter()
                        .any(|&[base, top]| {
                            top > feet + body.step + 0.25 && base < feet + body.height
                        })
                })
            {
                return (ahead - 1.0).max(0.0);
            }
            if floor >= feet - 2.0 {
                feet = floor;
            }
        }
        distance
    }

    pub fn clear(&self, start: Position, end: Position, gravity: f64, flight: f64) -> bool {
        let [dx, dy, dz] = std::array::from_fn(|i| end[i] - start[i]);
        let steps = (dx.hypot(dy).hypot(dz) / 2.0)
            .max(flight * 16.0)
            .max(2.0)
            .ceil() as u32;
        let arc = 0.5 * gravity * flight * flight;
        (1..steps).all(|i| {
            let t = f64::from(i) / f64::from(steps);
            let point = [start[0] + dx * t, start[1] + dy * t];
            let z = start[2] + dz * t + arc * t * (1.0 - t);
            z >= self.ground(point)
                && self
                    .spans(point)
                    .iter()
                    .all(|&[base, top]| z <= base || z >= top)
        })
    }

    #[cfg(test)]
    pub fn flat_test(solids: impl IntoIterator<Item = ((i32, i32), [f64; 2])>) -> Self {
        Self {
            bounds: [-100.0, 100.0, -100.0, 100.0],
            origin: [-100.0, -100.0],
            step: 200.0,
            scale: 0.01,
            width: 2,
            height: 2,
            terrain: vec![0; 4],
            solids: solids
                .into_iter()
                .map(|(cell, span)| (cell, vec![span]))
                .collect(),
        }
    }
}
