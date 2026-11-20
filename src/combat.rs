use crate::{
    angle_delta, arena::Arena, bearing, distance, flat, items, number, xy, xyz, Point, Position,
};
use serde_json::Value;

struct Shot {
    slot: usize,
    name: String,
    aim: Point,
    tolerance: Point,
    trigger: bool,
    score: f64,
}

struct Target<'a> {
    value: &'a Value,
    center: Position,
    height: f64,
    radius: f64,
}

struct Gunner<'a> {
    arena: &'a Arena,
    config: &'a Value,
    me: &'a Value,
    moving_fast: bool,
}

impl Gunner<'_> {
    fn shot(&self, target: &Target, weapon: &Value, slot: usize) -> Option<Shot> {
        let name = weapon["kind"].as_str()?;
        let w = &weapon["stats"];
        let charge = number(&weapon["charge"]);
        let ready = number(&weapon["cooldown"]) <= 0.10;
        if (name != "gun" && !ready && charge <= 0.0)
            || (name == "laser" && self.moving_fast && charge <= 0.0)
            || (name == "lightning"
                && number(&self.me["energy"])
                    < number(&w["energy"]) + if self.moving_fast { 26.0 } else { 10.0 })
        {
            return None;
        }
        let [x, y, z] = xyz(&self.me["pos"]);
        let stats = &self.me["stats"];
        let barrel = number(&stats["radius"]) + 0.5;
        let muzzle = if name == "mortar" {
            number(&stats["height"]) + 0.5
        } else {
            number(&stats["muzzle_height"])
        };
        let origin = [x, y, z + muzzle];
        let speed = number(&w["speed"]);
        let gravity = number(&w["gravity"]);
        let vel = xyz(&target.value["vel"]);
        let mut flight = 0.0;
        if speed > 0.0 {
            for _ in 0..5 {
                let [ix, iy, iz] = std::array::from_fn(|i| target.center[i] + vel[i] * flight);
                let reach = (ix - x)
                    .hypot(iy - y)
                    .hypot(iz - origin[2] + 0.5 * gravity * flight * flight);
                flight = (reach - barrel).max(0.0) / speed;
                if name == "mortar" && flight > 1.65 {
                    return None;
                }
            }
        }
        let impact = std::array::from_fn(|i| target.center[i] + vel[i] * flight);
        let horizontal = distance([x, y], flat(impact));
        let range = horizontal.hypot(impact[2] - origin[2]);
        if (name == "mortar" && range < number(&w["splash_radius"]) + barrel + 2.0)
            || (name != "mortar" && range > number(&w["range"]) - 0.5)
        {
            return None;
        }
        let aim = [
            bearing([x, y], flat(impact)),
            (impact[2] + 0.5 * gravity * flight * flight - origin[2]).atan2(horizontal),
        ];
        if aim[1] < number(&stats["min_pitch"])
            || aim[1] > number(&stats["max_pitch"])
            || !self.arena.clear(origin, impact, gravity, flight)
        {
            return None;
        }

        let travel = (horizontal - barrel).max(1.0);
        let tolerance = if name == "lightning" {
            [
                number(&w["cone_deg"]).to_radians() * 0.45,
                std::f64::consts::PI,
            ]
        } else {
            [
                (target.radius * 0.82).atan2(travel),
                (target.height * 0.34).atan2(travel),
            ]
        };
        let current = xy(&self.me["aim"]);
        let yaw_error = angle_delta(aim[0], current[0]).abs();
        let pitch_error = (aim[1] - current[1]).abs();
        let aligned = yaw_error < tolerance[0] && pitch_error < tolerance[1];
        let can_fire =
            name != "gun" || (number(&weapon["ammo"]) > 0.0 && number(&weapon["reload"]) <= 0.05);
        let trigger = match name {
            "laser" if charge > 0.0 => {
                !self.moving_fast && (charge < number(&w["charge"]) || !aligned)
            }
            "laser" => ready && yaw_error < 0.24,
            _ => ready && aligned && can_fire,
        };

        let mut score = match name {
            "gun" => 34.0,
            "mortar" => 52.0,
            "laser" => 50.0,
            "lightning" => 55.0,
            _ => return None,
        };
        score += if aligned { 15.0 } else { 0.0 } - 24.0 * yaw_error - 12.0 * pitch_error;
        if !ready || !can_fire {
            score -= 26.0;
        }
        if charge > 0.0 {
            score += 75.0;
        }
        let cubes = &self.config["cubes"];
        let damage = number(&w["damage"])
            * (1.0
                + number(&cubes["damage_per_cube"])
                    * number(&self.me["cubes"]).min(number(&cubes["stat_cap"])));
        if target.value["hp"].as_f64().is_some_and(|hp| hp <= damage) {
            score += 18.0;
        }
        Some(Shot {
            slot,
            name: name.to_owned(),
            aim,
            tolerance,
            trigger,
            score,
        })
    }
}

pub fn attack(
    arena: &Arena,
    config: &Value,
    me: &Value,
    targets: &[&Value],
    fast: bool,
    attacker: Option<u64>,
) -> Option<(Point, [bool; 2])> {
    let gunner = Gunner {
        arena,
        config,
        me,
        moving_fast: fast,
    };
    let pos = xy(&me["pos"]);
    let mut best: Option<(f64, Point, [bool; 2])> = None;
    for &value in targets.iter().take(5) {
        let kind = value["kind"].as_str().unwrap_or("crate");
        let geometry = &config[kind_for_geometry(kind)];
        let size = 1.0 + number(&value["cubes"]) * number(&config["cubes"]["size_per_cube"]);
        let height = number(&geometry["height"]) * size;
        let radius = number(&geometry["radius"]) * size;
        let [x, y, z] = xyz(&value["pos"]);
        let target = Target {
            value,
            center: [x, y, z + height * 0.5],
            height,
            radius,
        };
        let range = distance(pos, [x, y]);
        if range < 0.1 {
            continue;
        }
        let shots: Vec<_> = items(&me["weapons"])
            .iter()
            .take(2)
            .enumerate()
            .filter_map(|(slot, w)| gunner.shot(&target, w, slot))
            .collect();
        for shot in &shots {
            let mut fire = [false; 2];
            fire[shot.slot] = shot.trigger;
            let mut score = shot.score - 0.14 * range;
            for other in &shots {
                if other.slot == shot.slot {
                    continue;
                }
                let separate_arc = (shot.name == "gun" && other.name == "mortar")
                    || (shot.name == "mortar" && other.name == "gun");
                if !separate_arc
                    && angle_delta(shot.aim[0], other.aim[0]).abs() < other.tolerance[0]
                    && (shot.aim[1] - other.aim[1]).abs() < other.tolerance[1]
                {
                    fire[other.slot] = other.trigger;
                    if shot.trigger && other.trigger {
                        score += 12.0;
                    }
                }
            }
            score += match kind {
                "crate" => -20.0,
                "npc" => -12.0,
                _ => 0.0,
            };
            if attacker.is_some() && attacker == value["id"].as_u64() {
                score += 9.0;
            }
            if kind == "mech" {
                if let Some(hp) = value["hp"].as_f64() {
                    score += 34.0 * (1.0 - hp / 850.0).clamp(0.0, 1.0);
                }
            }
            if best.as_ref().is_none_or(|(old, _, _)| score > *old) {
                best = Some((score, shot.aim, fire));
            }
        }
    }
    best.map(|(_, aim, fire)| (aim, fire))
}

fn kind_for_geometry(kind: &str) -> &str {
    if kind == "crate" {
        "crates"
    } else {
        kind
    }
}

enum Approach {
    Bullet(Point),
    Blast(f64),
}

pub struct Hazard {
    relative: Point,
    approach: Approach,
    eta: f64,
}

impl Hazard {
    pub fn dodge_heading(&self, side: f64) -> f64 {
        match self.approach {
            Approach::Bullet([vx, vy]) => vy.atan2(vx) + side * std::f64::consts::FRAC_PI_2,
            Approach::Blast(_) => (-self.relative[1]).atan2(-self.relative[0]),
        }
    }
}

pub fn incoming(arena: &Arena, config: &Value, me: &Value, projectiles: &Value) -> Vec<Hazard> {
    let pos = xyz(&me["pos"]);
    let [x, y, z] = pos;
    let height = number(&me["stats"]["height"]);
    let mut hazards = Vec::new();
    for shot in items(projectiles) {
        let [px, py, pz] = xyz(&shot["pos"]);
        let [vx, vy, vz] = xyz(&shot["vel"]);
        let (rx, ry) = (px - x, py - y);
        if shot["kind"] == "mortar" {
            let mortar = &config["weapons"]["mortar"];
            if rx.hypot(ry) > 100.0 {
                continue;
            }
            let gravity = number(&mortar["gravity"]);
            let radius = number(&mortar["splash_radius"]);
            let mut previous = [px, py, pz];
            for i in 1..=12 {
                let t = f64::from(i) * 0.075;
                let point = [
                    px + vx * t,
                    py + vy * t,
                    pz + vz * t - 0.5 * gravity * t * t,
                ];
                let ground = arena.ground(flat(point));
                let blocked = arena
                    .spans(flat(point))
                    .iter()
                    .any(|&[base, top]| point[2] >= base && point[2] <= top);
                if point[2] <= ground || blocked {
                    let impact = if blocked {
                        previous
                    } else {
                        [point[0], point[1], ground + 0.05]
                    };
                    if distance(flat(impact), flat(pos)) < radius + 12.0
                        && (impact[2] - z).abs() < height + radius
                        && arena.clear(impact, [x, y, z + height * 0.5], 0.0, 0.0)
                    {
                        hazards.push(Hazard {
                            relative: [impact[0] - x, impact[1] - y],
                            approach: Approach::Blast(radius),
                            eta: t,
                        });
                    }
                    break;
                }
                previous = point;
            }
            continue;
        }
        let speed_sq = vx * vx + vy * vy;
        if speed_sq < 1.0 {
            continue;
        }
        let eta = -(rx * vx + ry * vy) / speed_sq;
        let impact_z = pz + vz * eta;
        if eta > 0.0
            && eta < 0.55
            && (rx + vx * eta).hypot(ry + vy * eta) < 10.0
            && impact_z > z - 1.0
            && impact_z < z + height + 1.0
        {
            hazards.push(Hazard {
                relative: [rx, ry],
                approach: Approach::Bullet([vx, vy]),
                eta,
            });
        }
    }
    hazards.sort_unstable_by(|a, b| a.eta.total_cmp(&b.eta));
    hazards.truncate(6);
    hazards
}

pub fn dodge_cost(me: &Value, [dx, dy]: Point, hazards: &[Hazard]) -> f64 {
    let [vx, vy] = xy(&me["vel"]);
    let speed = number(&me["stats"]["walk_speed"]);
    let buffer = number(&me["stats"]["radius"]) + 1.4;
    hazards
        .iter()
        .map(|shot| {
            let [rx, ry] = shot.relative;
            let blend = (shot.eta * 2.5).min(0.85);
            let x = vx * (1.0 - blend) + dx * speed * blend;
            let y = vy * (1.0 - blend) + dy * speed * blend;
            match shot.approach {
                Approach::Bullet([sx, sy]) => {
                    let (rel_x, rel_y) = (sx - x, sy - y);
                    let t = (-(rx * rel_x + ry * rel_y) / (rel_x * rel_x + rel_y * rel_y + 0.01))
                        .clamp(0.0, 0.55);
                    8.0 * (buffer - (rx + rel_x * t).hypot(ry + rel_y * t)).max(0.0)
                }
                Approach::Blast(radius) => {
                    let miss = (rx - x * shot.eta).hypot(ry - y * shot.eta);
                    7.0 * (radius + 1.5 - miss).max(0.0)
                }
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dodge_prefers_lateral_motion_on_incoming_shot() {
        let me = json!({"pos":[0,0,0],"vel":[0,0,0],"stats":{"radius":2.5,"height":9,"walk_speed":11.7}});
        let hazard = incoming(
            &Arena::flat_test([]),
            &json!({}),
            &me,
            &json!([{"kind":"gun","pos":[-80,0,4],"vel":[252,0,0]}]),
        );
        assert_eq!(hazard.len(), 1);
        assert!(dodge_cost(&me, [0.0, 1.0], &hazard) < dodge_cost(&me, [1.0, 0.0], &hazard));
    }

    #[test]
    fn avoids_predicted_mortar_blast() {
        let me = json!({"pos":[0,0,0],"vel":[0,0,0],"stats":{"radius":2.5,"height":9,"walk_speed":11.7}});
        let config = json!({"weapons":{"mortar":{"gravity":22,"splash_radius":7}}});
        let shot = json!([{"kind":"mortar","pos":[-38,0,4],"vel":[72,0,-2]}]);
        let hazards = incoming(&Arena::flat_test([]), &config, &me, &shot);
        assert_eq!(hazards.len(), 1);
        assert!(dodge_cost(&me, [1.0, 0.0], &hazards) < dodge_cost(&me, [-1.0, 0.0], &hazards));
    }
}
