use crate::{
    angle_delta,
    arena::{Arena, Body},
    bearing,
    combat::{self, Hazard},
    distance, flat, items, number, offset,
    routing::{Router, Zone},
    unit, xy, xyz, Point, Position,
};
use serde_json::{json, Value};
use std::{
    f64::consts::{PI, TAU},
    io,
    path::Path,
};

const UPGRADES: &[&str] = &[
    "nanites",
    "servos",
    "plating",
    "armor",
    "jets",
    "reactor",
    "capacitor",
    "gun_mag",
    "laser_charge",
    "gun_damage",
    "lightning_efficiency",
    "lightning_range",
    "gun_rate",
    "gun_velocity",
    "laser_damage",
    "mortar_reload",
    "mortar_splash",
    "mortar_velocity",
    "gyros",
    "sensors",
];

pub struct Bot {
    config: Value,
    arena: Arena,
    router: Router,
    last_move: Point,
    attacked_until: f64,
    attacker: Option<u64>,
    attacker_yaw: f64,
    watch: Option<(Point, f64)>,
    detour: Option<(f64, f64)>,
    side: f64,
    jet_until: f64,
    jet_retry: f64,
    sprinting: bool,
}

struct Movement<'a> {
    me: &'a Value,
    goal: Point,
    zone: Zone,
    now: f64,
    away: Option<f64>,
    urgent: bool,
    routed: bool,
    hazards: &'a [Hazard],
    cover_from: Option<Position>,
}

impl Bot {
    pub fn new(hello: Value) -> io::Result<Self> {
        let path = hello["map"]["dir"].as_str().unwrap_or("/arena");
        let arena = Arena::load(Path::new(path))?;
        Ok(Self {
            config: hello["config"].clone(),
            arena,
            router: Router::new(),
            last_move: [0.0; 2],
            attacked_until: -1.0,
            attacker: None,
            attacker_yaw: 0.0,
            watch: None,
            detour: None,
            side: if hello["seed"].as_u64().unwrap_or_default() % 2 == 1 {
                1.0
            } else {
                -1.0
            },
            jet_until: -1.0,
            jet_retry: -1.0,
            sprinting: false,
        })
    }

    pub fn act(&mut self, obs: &Value) -> Value {
        let me = &obs["self"];
        let stats = &me["stats"];
        let pos = xyz(&me["pos"]);
        let start = flat(pos);
        let now = number(&obs["t"]);
        let data = &obs["zone"];
        let zone = Zone {
            center: xy(&data["center"]),
            radius: number(&data["radius"]),
            next: xy(&data["next_center"]),
            next_radius: number(&data["next_radius"]),
            shrink_end: number(&data["shrink_end"]),
        };
        let mut action = json!({"tick":obs["tick"]});
        if let Some((i, _)) = items(&obs["offer"]["choices"])
            .iter()
            .enumerate()
            .min_by_key(|(_, choice)| {
                let id = choice["id"].as_str().unwrap_or_default();
                UPGRADES
                    .iter()
                    .position(|&name| id == name)
                    .unwrap_or(UPGRADES.len())
            })
        {
            action["pick"] = json!(i);
        }

        for event in items(&obs["events"]) {
            if event["type"] == "damaged" {
                if let Some(yaw) = event["bearing"].as_f64() {
                    self.attacker_yaw = yaw;
                    self.attacker = event["by"].as_u64();
                    self.attacked_until = now + 2.0;
                }
            }
        }
        let attacked = now < self.attacked_until;
        let injured = number(&me["hp"]) < number(&me["max_hp"]) * 0.57;
        let recovering = injured && !attacked;
        let nearby_fighting = items(&obs["sounds"]).iter().any(|sound| {
            sound["dist"] == "near"
                && matches!(
                    sound["kind"].as_str(),
                    Some("gun" | "mortar" | "laser" | "laser_charge" | "lightning" | "boost")
                )
        });

        let mut actors: Vec<_> = items(&obs["mechs"]).iter().collect();
        actors.sort_by(|a, b| range(start, a).total_cmp(&range(start, b)));
        let pressure: Vec<_> = actors
            .iter()
            .copied()
            .filter(|enemy| {
                range(start, enemy) < if enemy["kind"] == "mech" { 100.0 } else { 48.0 }
            })
            .collect();
        let attacker = attacked.then_some(self.attacker).flatten().and_then(|id| {
            actors
                .iter()
                .copied()
                .find(|actor| actor["id"].as_u64() == Some(id))
        });
        let threat = attacker.or(pressure.first().copied());
        let danger = threat.is_some() || attacked;
        let safe_radius = zone.next_radius - (zone.next_radius * 0.25).min(22.0);
        let future_gap = distance(start, zone.next) - safe_radius;
        let outside_buffer = distance(start, zone.center) > (zone.radius - 10.0).max(0.0);
        let walk = number(&stats["walk_speed"]);
        let urgent = me["in_zone"] == false
            || 1.35 * future_gap > walk * (zone.shrink_end - now - 10.0).max(0.0);
        let mut zone_destination = future_gap > 0.0 || outside_buffer || zone.next_radius == 0.0;
        let mut goal = if zone_destination { zone.next } else { start };

        let pickup = best_pickup(start, pos[2], me, &obs["pickups"], &self.config);
        let reach = number(&self.config["loot"]["pickup_radius"]);
        if let Some((item, slot)) = pickup {
            if range(start, item) <= reach {
                action["take"] = item["id"].clone();
                if let Some(slot) = slot {
                    action["drop"] = json!(slot);
                }
            }
        }
        let safe = |item: &Value| {
            distance(xy(&item["pos"]), zone.next) <= safe_radius
                && (number(&item["pos"][2]) - pos[2]).abs() <= 4.0
        };
        let closest = |a: &&Value, b: &&Value| range(start, a).total_cmp(&range(start, b));
        let mut loot_target = None;
        if !urgent && !danger {
            let drop = items(&obs["drops"])
                .iter()
                .filter(|drop| {
                    (drop["holder"].is_null() || drop["holder"] == me["id"])
                        && safe(drop)
                        && number(&drop["lands_in"]) < range(start, drop) / walk.max(0.1) + 1.5
                        && range(start, drop) < if injured { 95.0 } else { 65.0 }
                })
                .min_by(closest);
            let cube = items(&obs["cubes"])
                .iter()
                .filter(|cube| safe(cube) && range(start, cube) < 48.0)
                .min_by(closest);
            let crate_target = items(&obs["crates"])
                .iter()
                .filter(|item| safe(item) && range(start, item) < 52.0)
                .min_by(closest);
            let farm = !injured
                && !nearby_fighting
                && number(&me["cubes"]) < number(&self.config["cubes"]["stat_cap"]);
            let objective = if let Some(drop) = drop.filter(|_| !nearby_fighting || injured) {
                Some(if range(start, drop) < number(&drop["radius"]) * 0.65 {
                    start
                } else {
                    xy(&drop["pos"])
                })
            } else if let Some(cube) = cube {
                Some(xy(&cube["pos"]))
            } else if let Some((item, _)) =
                pickup.filter(|(item, _)| safe(item) && range(start, item) > reach)
            {
                Some(xy(&item["pos"]))
            } else if let Some(item) = crate_target.filter(|_| farm) {
                loot_target = Some(item);
                let dist = range(start, item);
                Some(if dist > 13.0 {
                    xy(&item["pos"])
                } else if dist < 8.0 {
                    offset(start, unit(bearing(xy(&item["pos"]), start)), 5.0)
                } else {
                    start
                })
            } else {
                None
            };
            if let Some(point) = objective {
                goal = point;
                zone_destination = false;
            }
        }

        let finish = threat.filter(|enemy| {
            !urgent
                && !outside_buffer
                && !nearby_fighting
                && pressure.len() == 1
                && distance(start, zone.next) < safe_radius
                && can_finish(me, enemy, start)
        });
        let fleeing = finish.is_none()
            && (attacked
                || threat.is_some_and(|enemy| {
                    injured || pressure.len() > 1 || range(start, enemy) < 65.0
                }));
        if let Some(enemy) = finish {
            let enemy_pos = xy(&enemy["pos"]);
            let toward = bearing(start, enemy_pos);
            let radial = ((range(start, enemy) - 29.0) * 0.55).clamp(-10.0, 10.0);
            goal = offset(
                offset(start, unit(toward), radial),
                unit(toward + self.side * PI / 2.0),
                11.0,
            );
            zone_destination = false;
        }
        let away = fleeing.then(|| {
            threat.map_or(self.attacker_yaw, |enemy| bearing(start, xy(&enemy["pos"]))) + PI
        });
        if let Some(yaw) = away {
            if !urgent && zone.next_radius > 0.0 {
                goal = escape_goal(start, yaw, zone, safe_radius);
                zone_destination = false;
            }
        }
        let hazards = combat::incoming(&self.arena, &self.config, me, &obs["projectiles"]);
        if let Some(shot) = hazards.first().filter(|_| distance(start, goal) < 3.0) {
            goal = offset(start, unit(shot.dodge_heading(self.side)), 8.0);
            zone_destination = false;
        }
        let body = Body::from_stats(stats);
        let region = if zone_destination && !outside_buffer && safe_radius > 15.0 {
            safe_radius - (safe_radius * 0.22).min(24.0)
        } else {
            0.0
        };
        let (waypoint, routed) =
            self.router
                .waypoint(&self.arena, pos, (goal, region), body, zone, now);
        let cover_from = threat.filter(|_| fleeing).map(|value| xyz(&value["pos"]));
        let (movement, mut jump) = self.navigate(
            Movement {
                me,
                goal: waypoint,
                zone,
                now,
                away,
                urgent,
                routed,
                hazards: &hazards,
                cover_from,
            },
            body,
        );
        self.last_move = movement;
        let grounded = me["grounded"] == true;
        let battery = number(&me["energy"]) / number(&me["max_energy"]).max(1.0);
        if urgent
            && !routed
            && self.detour.is_some()
            && grounded
            && now >= self.jet_retry
            && battery > 0.55
        {
            jump = true;
            self.jet_until = now + 1.4;
            self.jet_retry = now + 7.0;
        }
        let reserve = if urgent { 0.14 } else { 0.28 };
        let boost = if jump || !grounded {
            self.sprinting = false;
            battery > reserve && (jump || now < self.jet_until)
        } else {
            self.sprinting = (urgent || fleeing)
                && magnitude(movement) >= 0.75
                && battery
                    > if self.sprinting {
                        reserve
                    } else {
                        reserve + 0.30
                    };
            self.sprinting
        };
        action["move"] = json!(movement);
        action["jump"] = json!(jump);
        action["boost"] = json!(boost);

        let mut targets: Vec<_> = if let Some(enemy) = finish {
            vec![enemy]
        } else if let Some(item) = loot_target {
            vec![item]
        } else {
            actors
                .into_iter()
                .filter(|actor| {
                    if recovering {
                        range(start, actor) < 33.0
                    } else {
                        actor["kind"] == "mech" || range(start, actor) < 65.0
                    }
                })
                .take(4)
                .collect()
        };
        if let Some(enemy) = attacker.filter(|enemy| !recovering && !targets.contains(enemy)) {
            targets.insert(0, enemy);
        }
        if let Some((aim, fire)) = combat::attack(
            &self.arena,
            &self.config,
            me,
            &targets,
            fleeing || urgent,
            self.attacker,
        ) {
            action["aim"] = json!(aim);
            action["fire"] = json!(fire);
        } else {
            let yaw = if attacked {
                self.attacker_yaw
            } else if let Some(enemy) = threat {
                bearing(start, xy(&enemy["pos"]))
            } else if magnitude(movement) > 0.05 {
                movement[1].atan2(movement[0])
            } else {
                (now * 1.1 + PI).rem_euclid(TAU) - PI
            };
            action["aim"] = json!([yaw, 0.0]);
        }
        if !danger
            && loot_target.is_none()
            && items(&me["weapons"]).iter().any(|w| {
                let ammo = number(&w["ammo"]);
                w["kind"] == "gun" && ammo > 0.0 && ammo <= number(&w["stats"]["mag"]) / 3.0
            })
        {
            action["reload"] = json!(true);
        }
        action
    }

    fn navigate(&mut self, context: Movement, body: Body) -> (Point, bool) {
        let Movement {
            me,
            goal,
            zone,
            now,
            away,
            urgent,
            routed,
            hazards,
            cover_from,
        } = context;
        let pos = xyz(&me["pos"]);
        let start = flat(pos);
        let dist = distance(start, goal);
        if self.watch.is_none_or(|(_, time)| now - time >= 0.8) {
            if let Some((previous, _)) = self.watch {
                if dist > 5.0 && magnitude(self.last_move) > 0.5 && distance(start, previous) < 1.5
                {
                    let yaw = self
                        .detour
                        .map_or(bearing(start, goal) + self.side * PI / 2.0, |(_, yaw)| yaw);
                    self.detour = Some((now + 2.5, yaw));
                }
            }
            self.watch = Some((start, now));
        }
        if routed || self.detour.is_some_and(|(until, _)| now >= until) {
            self.detour = None;
        }
        if dist < 0.65 {
            return ([0.0; 2], false);
        }
        let direct = bearing(start, goal);
        let heading = self.detour.map_or(direct, |(_, yaw)| yaw);
        let probe = dist.clamp(2.5, 11.0);
        let mut candidates = Vec::with_capacity(12);
        for degrees in [
            0.0_f64, 25.0, -25.0, 50.0, -50.0, 75.0, -75.0, 105.0, -105.0, 145.0, -145.0, 180.0,
        ] {
            let yaw = heading + degrees.to_radians();
            let direction = unit(yaw);
            let clear = self.arena.travel(pos, direction, probe, body);
            if clear < 2.5 {
                continue;
            }
            let end = offset(start, direction, clear);
            let progress = dist - distance(end, goal);
            let overflow = (distance(end, zone.center) - (zone.radius - 3.0).max(0.0)).max(0.0);
            let next_overflow = (distance(end, zone.next) - zone.next_radius).max(0.0);
            let mut score = progress * if self.detour.is_some() { 0.6 } else { 1.0 } + 0.4 * clear
                - 2.4 * overflow
                - 0.45 * next_overflow;
            score += 1.8 * (direction[0] * self.last_move[0] + direction[1] * self.last_move[1]);
            if self.detour.is_some() {
                score += 5.0 * angle_delta(yaw, heading).cos();
            }
            if let Some(away) = away {
                score += clear
                    * angle_delta(yaw, away).cos()
                    * if urgent { 0.3 } else { 1.0 }
                    * if routed { 0.35 } else { 1.0 };
            }
            score -= combat::dodge_cost(me, direction, hazards) * if urgent { 0.5 } else { 1.0 };
            candidates.push((score, direction, clear, end));
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        let Some(&first) = candidates.first() else {
            return (
                unit(heading),
                me["grounded"] == true && number(&me["energy"]) > 0.35 * number(&me["max_energy"]),
            );
        };
        let mut best = first;
        if let Some([x, y, z]) = cover_from.filter(|_| !urgent) {
            let stats = &me["stats"];
            let origin = [x, y, z + number(&stats["muzzle_height"])];
            for &(score, dir, clear, end) in candidates.iter().take(4) {
                if score < first.0 - 6.0 {
                    break;
                }
                let feet = self.arena.ground(end).max(pos[2] - 3.0);
                let eye = [end[0], end[1], feet + number(&stats["eye_height"])];
                let chest = [end[0], end[1], feet + body.height * 0.5];
                if !self.arena.clear(origin, eye, 0.0, 0.0)
                    && !self.arena.clear(origin, chest, 0.0, 0.0)
                    && score + 6.0 > best.0
                {
                    best = (score + 6.0, dir, clear, end);
                }
            }
        }
        let speed = (dist / 3.0).min(best.2 / 4.0).min(1.0);
        (best.1.map(|v| v * speed), false)
    }
}

fn range(start: Point, item: &Value) -> f64 {
    distance(start, xy(&item["pos"]))
}
fn magnitude([x, y]: Point) -> f64 {
    x.hypot(y)
}

// Close out a weakened opponent only while healthy and already inside the safe ring.
fn can_finish(me: &Value, enemy: &Value, start: Point) -> bool {
    let max_hp = enemy["max_hp"].as_f64().unwrap_or(1400.0);
    enemy["kind"] == "mech"
        && range(start, enemy) < 50.0
        && number(&me["hp"]) >= 0.72 * number(&me["max_hp"])
        && enemy["hp"]
            .as_f64()
            .is_some_and(|hp| hp > 0.0 && hp < (max_hp * 0.32).min(420.0))
}

fn escape_goal(pos: Point, heading: f64, zone: Zone, safe_radius: f64) -> Point {
    let mut best = (f64::NEG_INFINITY, pos);
    for degrees in [0.0_f64, 45.0, -45.0, 90.0, -90.0, 135.0, -135.0, 180.0] {
        let angle = degrees.to_radians();
        let goal = offset(pos, unit(heading + angle), 28.0);
        let overflow = (distance(goal, zone.center) - zone.radius + 5.0).max(0.0)
            + 0.6 * (distance(goal, zone.next) - safe_radius).max(0.0);
        let score = 15.0 * angle.cos() - 2.0 * overflow;
        if score > best.0 {
            best = (score, goal);
        }
    }
    best.1
}

fn weapon_rank(kind: &Value) -> u32 {
    match kind.as_str() {
        Some("mortar") => 1,
        Some("laser") => 2,
        Some("lightning") => 3,
        _ => 0,
    }
}

fn best_pickup<'a>(
    start: Point,
    z: f64,
    me: &Value,
    pickups: &'a Value,
    config: &Value,
) -> Option<(&'a Value, Option<usize>)> {
    let held = items(&me["weapons"]);
    let empty = held.iter().any(Value::is_null);
    let swap = if empty {
        None
    } else {
        let (slot, weapon) = held
            .iter()
            .enumerate()
            .filter(|(_, w)| weapon_rank(&w["kind"]) > 0 && number(&w["charge"]) == 0.0)
            .min_by_key(|(_, w)| weapon_rank(&w["kind"]))?;
        Some((slot, weapon_rank(&weapon["kind"])))
    };
    let reach = number(&config["loot"]["pickup_radius"]);
    let item = items(pickups)
        .iter()
        .filter(|item| {
            let rank = weapon_rank(&item["kind"]);
            rank > swap.map_or(0, |(_, rank)| rank)
                && range(start, item) <= if empty { 48.0 } else { 30.0 }
                && (number(&item["pos"][2]) - z).abs() <= 4.0
                && !held.iter().any(|weapon| weapon["kind"] == item["kind"])
        })
        .min_by(|a, b| {
            let cost = |item: &Value| {
                (range(start, item) - reach).max(0.0) - 4.0 * f64::from(weapon_rank(&item["kind"]))
            };
            cost(a).total_cmp(&cost(b))
        })?;
    Some((item, swap.map(|(slot, _)| slot)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_does_not_ask_to_leave_the_current_ring() {
        let zone = Zone {
            center: [0.0, 0.0],
            radius: 80.0,
            next: [0.0, 0.0],
            next_radius: 60.0,
            shrink_end: 100.0,
        };
        assert!(distance(escape_goal([75.0, 0.0], 0.0, zone, 50.0), zone.center) < zone.radius);
    }

    #[test]
    fn pickup_rejects_roofs_and_charging_laser_swaps() {
        let config = json!({"loot":{"pickup_radius":4}});
        let weapons = json!({"weapons":[{"kind":"gun"},null]});
        let pickups = json!([{"id":1,"kind":"lightning","pos":[1,0,10]},{"id":2,"kind":"laser","pos":[2,0,0]}]);
        assert_eq!(
            best_pickup([0.0, 0.0], 0.0, &weapons, &pickups, &config)
                .unwrap()
                .0["id"],
            2
        );
        let weapons = json!({"weapons":[{"kind":"gun"},{"kind":"laser","charge":0.3}]});
        assert!(best_pickup([0.0, 0.0], 0.0, &weapons, &pickups, &config).is_none());
    }

    #[test]
    fn finishing_requires_a_healthy_mech_and_a_visible_low_hp_enemy() {
        let me = json!({"hp":1100,"max_hp":1400});
        let enemy = json!({"kind":"mech","hp":260,"max_hp":1400,"pos":[40,0,0]});
        assert!(can_finish(&me, &enemy, [0.0, 0.0]));
        let injured = json!({"hp":650,"max_hp":1400});
        assert!(!can_finish(&injured, &enemy, [0.0, 0.0]));
        let unknown = json!({"kind":"mech","pos":[40,0,0]});
        assert!(!can_finish(&me, &unknown, [0.0, 0.0]));
    }
}
