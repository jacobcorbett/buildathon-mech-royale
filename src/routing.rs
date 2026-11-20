use crate::{
    arena::{Arena, Body},
    distance, flat, offset, Point, Position,
};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, VecDeque},
};

const SPACING: f64 = 4.0;
const MOVES: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];
type Cell = (i32, i32);

fn cell([x, y]: Point) -> Cell {
    ((x / SPACING).round() as i32, (y / SPACING).round() as i32)
}
fn point((x, y): Cell) -> Point {
    [f64::from(x) * SPACING, f64::from(y) * SPACING]
}
fn direction(from: Point, to: Point, gap: f64) -> Point {
    [(to[0] - from[0]) / gap, (to[1] - from[1]) / gap]
}

#[derive(Clone, Copy)]
pub struct Zone {
    pub center: Point,
    pub radius: f64,
    pub next: Point,
    pub next_radius: f64,
    pub shrink_end: f64,
}

pub struct Router {
    goal: Option<(Point, f64)>,
    profile: (i32, i32),
    walkable: HashMap<Cell, bool>,
    pending: Option<Search>,
    route: VecDeque<Point>,
    retry_at: f64,
}

struct Search {
    from: Cell,
    dest: Cell,
    region: f64,
    zone: Zone,
    fringe: BinaryHeap<Reverse<(u32, u32, Cell)>>,
    costs: HashMap<Cell, u32>,
    parents: HashMap<Cell, Cell>,
    expanded: usize,
}

impl Search {
    fn heuristic(&self, at: Cell) -> u32 {
        ((distance(point(at), point(self.dest)) - self.region).max(0.0) * 29.0) as u32
    }

    fn path(&self, mut at: Cell) -> VecDeque<Point> {
        let mut result = VecDeque::new();
        while at != self.from {
            result.push_front(point(at));
            at = self.parents[&at];
        }
        result
    }
}

impl Router {
    pub fn new() -> Self {
        Self {
            goal: None,
            profile: (0, 0),
            walkable: HashMap::new(),
            pending: None,
            route: VecDeque::new(),
            retry_at: 0.0,
        }
    }

    fn is_walkable(&mut self, arena: &Arena, at: Cell, body: Body) -> bool {
        *self.walkable.entry(at).or_insert_with(|| {
            let center = point(at);
            if !arena.within(center, body.radius) {
                return false;
            }
            let ground = arena.ground(center);
            std::iter::once((0, 0)).chain(MOVES).all(|(x, y)| {
                let scale = if x != 0 && y != 0 {
                    std::f64::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                arena
                    .spans(offset(
                        center,
                        [f64::from(x), f64::from(y)],
                        body.radius * scale,
                    ))
                    .iter()
                    .all(|&[base, top]| top <= ground + body.step || base >= ground + body.height)
            })
        })
    }

    fn snap(&mut self, arena: &Arena, pos: Point, body: Body, limit: i32) -> Option<Cell> {
        let (x, y) = cell(pos);
        if self.is_walkable(arena, (x, y), body) {
            return Some((x, y));
        }
        for ring in 1..=limit {
            let nearest = (-ring..=ring)
                .flat_map(|dx| (-ring..=ring).map(move |dy| (dx, dy)))
                .filter(|(dx, dy)| dx.abs().max(dy.abs()) == ring)
                .map(|(dx, dy)| (x + dx, y + dy))
                .filter(|&at| self.is_walkable(arena, at, body))
                .min_by(|&a, &b| distance(point(a), pos).total_cmp(&distance(point(b), pos)));
            if nearest.is_some() {
                return nearest;
            }
        }
        None
    }

    fn advance(
        &mut self,
        arena: &Arena,
        search: &mut Search,
        body: Body,
    ) -> Option<VecDeque<Point>> {
        // Bound work per observation even on a disconnected part of the map.
        for _ in 0..180 {
            let Some(Reverse((_, cost, at))) = search.fringe.pop() else {
                return Some(VecDeque::new());
            };
            if search.costs[&at] != cost {
                continue;
            }
            if at == search.dest || distance(point(at), point(search.dest)) <= search.region {
                return Some(search.path(at));
            }
            search.expanded += 1;
            if search.expanded >= 4800 {
                return Some(VecDeque::new());
            }
            let start = point(at);
            let ground = arena.ground(start);
            for (dx, dy) in MOVES {
                let other = (at.0 + dx, at.1 + dy);
                if !self.is_walkable(arena, other, body) {
                    continue;
                }
                let diagonal = dx != 0 && dy != 0;
                if diagonal
                    && (!self.is_walkable(arena, (other.0, at.1), body)
                        || !self.is_walkable(arena, (at.0, other.1), body))
                {
                    continue;
                }
                let end = point(other);
                let length = distance(start, end);
                if (arena.ground(end) - ground).abs() > body.step.max(length * body.slope) {
                    continue;
                }
                let overflow =
                    (distance(end, search.zone.center) - search.zone.radius + 3.0).clamp(0.0, 40.0);
                let next = cost + if diagonal { 141 } else { 100 } + (10.0 * overflow) as u32;
                if next < *search.costs.get(&other).unwrap_or(&u32::MAX) {
                    search.costs.insert(other, next);
                    search.parents.insert(other, at);
                    search
                        .fringe
                        .push(Reverse((next + search.heuristic(other), next, other)));
                }
            }
        }
        None
    }

    pub fn waypoint(
        &mut self,
        arena: &Arena,
        pos: Position,
        (goal, region): (Point, f64),
        body: Body,
        zone: Zone,
        time: f64,
    ) -> (Point, bool) {
        let start = flat(pos);
        let gap = distance(start, goal);
        if gap < 3.0 || (pos[2] - arena.ground(start)).abs() > 2.5 {
            self.goal = None;
            self.pending = None;
            self.route.clear();
            return (goal, false);
        }
        let profile = ((body.radius * 2.0).ceil() as i32, body.height.ceil() as i32);
        if self.profile != profile {
            self.profile = profile;
            self.walkable.clear();
            self.goal = None;
        }
        if !self.goal.is_some_and(|(old, radius)| {
            distance(old, goal) <= 9.0 && (radius - region).abs() <= 4.0
        }) {
            self.goal = Some((goal, region));
            self.pending = None;
            self.route.clear();
            self.retry_at = time;
        }
        let heading = direction(start, goal, gap);
        let dist = if region > 0.0 {
            (gap - region).max(3.0)
        } else {
            gap
        };
        let destination = offset(start, heading, dist);
        let probe = dist.min(26.0);
        if (dist <= probe || (self.route.is_empty() && self.pending.is_none()))
            && arena.travel(pos, heading, probe, body) >= probe
        {
            self.pending = None;
            self.route.clear();
            return (destination, false);
        }
        let wide_body = Body {
            radius: f64::from(profile.0) / 2.0 + 0.5,
            height: f64::from(profile.1),
            ..body
        };
        if self.pending.is_none() && self.route.is_empty() && time >= self.retry_at {
            let from = self.snap(arena, start, wide_body, 2);
            let goal_cell = if region > 0.0 {
                Some(cell(goal))
            } else {
                self.snap(arena, goal, wide_body, 5)
            };
            if let (Some(from), Some(dest)) = (from, goal_cell) {
                if from != dest {
                    self.pending = Some(Search {
                        from,
                        dest,
                        region,
                        zone,
                        fringe: BinaryHeap::from([Reverse((0, 0, from))]),
                        costs: HashMap::from([(from, 0)]),
                        parents: HashMap::new(),
                        expanded: 0,
                    });
                }
            }
            self.retry_at = time + 1.5;
        }
        if let Some(mut pending) = self.pending.take() {
            if let Some(route) = self.advance(arena, &mut pending, wide_body) {
                self.route = route;
            } else {
                self.pending = Some(pending);
            }
        }
        while let Some(&step) = self.route.front() {
            if distance(start, step) >= 2.5 {
                break;
            }
            if region == 0.0 && self.route.len() == 1 && distance(step, goal) > SPACING {
                return (step, true);
            }
            self.route.pop_front();
        }
        for index in (1..self.route.len().min(6)).rev() {
            let step = self.route[index];
            let dist = distance(start, step);
            if (0.01..=18.0).contains(&dist)
                && arena.travel(pos, direction(start, step, dist), dist, body) >= dist
            {
                self.route.drain(..index);
                break;
            }
        }
        self.route
            .front()
            .map_or((destination, false), |&step| (step, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_can_be_reached_when_center_is_behind_a_full_wall() {
        let arena =
            Arena::flat_test((-50..50).flat_map(|y| (2..6).map(move |x| ((x, y), [0.0, 20.0]))));
        let mut router = Router::new();
        let body = Body {
            radius: 2.5,
            height: 9.0,
            step: 1.2,
            slope: 1.19,
        };
        let zone = Zone {
            center: [0.0, 0.0],
            radius: 85.0,
            next: [35.0, 0.0],
            next_radius: 45.0,
            shrink_end: 100.0,
        };
        let (goal, routed) = router.waypoint(
            &arena,
            [-30.0, 0.0, 0.0],
            (zone.next, 40.0),
            body,
            zone,
            0.0,
        );
        assert!(!routed);
        assert_eq!(goal, [-5.0, 0.0]);
    }

    #[test]
    fn routes_around_u_with_a_reversal() {
        let solids = (-12..12).flat_map(|y| {
            (-16..6).filter_map(move |x| {
                ((2..6).contains(&x) || !(-8..8).contains(&y)).then_some(((x, y), [0.0, 20.0]))
            })
        });
        let arena = Arena::flat_test(solids);
        let mut router = Router::new();
        let body = Body {
            radius: 2.5,
            height: 9.0,
            step: 1.2,
            slope: 1.19,
        };
        let zone = Zone {
            center: [0.0, 0.0],
            radius: 90.0,
            next: [36.0, 0.0],
            next_radius: 50.0,
            shrink_end: 120.0,
        };
        for tick in 0..30 {
            let (_, routed) = router.waypoint(
                &arena,
                [-12.0, 0.0, 0.0],
                (zone.next, 0.0),
                body,
                zone,
                f64::from(tick) / 10.0,
            );
            if routed {
                assert!(router.route.iter().any(|p| p[0] < -34.0));
                return;
            }
        }
        panic!("expected a ground detour");
    }
}
