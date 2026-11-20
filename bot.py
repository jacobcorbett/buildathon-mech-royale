"""Placement-first policy for the Mech Royale game."""
import math

from arena import Arena
import mechfight as mf


WEAPONS = ("gun", "mortar", "laser", "lightning")
UPGRADES = (
    "nanites", "servos", "plating", "armor", "jets", "reactor", "capacitor",
    "gun_mag", "laser_charge", "gun_damage", "lightning_efficiency", "lightning_range",
    "gun_rate", "gun_velocity", "laser_damage", "mortar_reload", "mortar_splash",
    "mortar_velocity", "gyros", "sensors",
)


def delta(a, b):
    return math.remainder(a - b, math.tau)


def bearing(a, b):
    return math.atan2(b[1] - a[1], b[0] - a[0])


def distance(a, b):
    return math.hypot(b[0] - a[0], b[1] - a[1])


def unit(yaw):
    return math.cos(yaw), math.sin(yaw)


class Bot:
    name = "Oval Runner"

    def __init__(self, hello):
        self.config = hello["config"]
        self.arena = Arena(hello["map"]["dir"])
        self.last_move = (0, 0)
        self.attacked_until = -1
        self.attacker_yaw = 0
        self.watch_position = None
        self.watch_time = 0
        self.detour_until = -1
        self.detour_side = 1 if hello["seed"] % 2 else -1
        self.detour_heading = None

    def act(self, obs):
        me = obs["self"]
        pos, stats = me["pos"], me["stats"]
        now, zone = obs["t"], obs["zone"]
        action = {"tick": obs["tick"]}
        if obs["offer"] and obs["offer"]["choices"]:
            ranks = {name: i for i, name in enumerate(UPGRADES)}
            action["pick"] = min(
                range(len(obs["offer"]["choices"])),
                key=lambda i: ranks.get(obs["offer"]["choices"][i]["id"], len(ranks)),
            )

        for event in obs["events"]:
            if event["type"] == "damaged" and event["bearing"] is not None:
                self.attacker_yaw = event["bearing"]
                self.attacked_until = now + 2.0
        actors = sorted(obs["mechs"], key=lambda enemy: distance(pos, enemy["pos"]))
        pressure = [
            enemy for enemy in actors
            if distance(pos, enemy["pos"]) < (100 if enemy["kind"] == "mech" else 48)
        ]
        attacked = now < self.attacked_until
        danger = bool(pressure or attacked)
        injured = me["hp"] < 0.57 * me["max_hp"]
        next_center = zone["next_center"]
        next_radius = zone["next_radius"]
        safe_radius = next_radius - min(22, next_radius * 0.25)
        remaining = max(0, zone["shrink_end"] - now)
        next_distance = distance(pos, next_center)
        outside_buffer = distance(pos, zone["center"]) > max(0, zone["radius"] - 10)
        urgent = (not me["in_zone"] or
                  next_distance - safe_radius > stats["walk_speed"] * max(0, remaining - 10))
        goal = next_center if next_distance > safe_radius or outside_buffer else pos[:2]
        if next_radius == 0:
            goal = next_center

        # Loot is worth a detour when it stays inside the coming circle.
        def safe(item):
            return (distance(item["pos"], next_center) <= safe_radius
                    and abs(item["pos"][2] - pos[2]) <= 4)

        pickup = self.pickup(obs)
        if pickup:
            item, slot = pickup
            if distance(pos, item["pos"]) <= self.config["loot"]["pickup_radius"]:
                action["take"] = item["id"]
                if slot is not None:
                    action["drop"] = slot

        drops = [
            drop for drop in obs["drops"]
            if drop["holder"] in (None, me["id"]) and drop["lands_in"] < 4
            and safe(drop) and distance(pos, drop["pos"]) < (80 if injured else 45)
        ]
        cubes = [cube for cube in obs["cubes"] if safe(cube) and distance(pos, cube["pos"]) < 32]
        crates = [crate for crate in obs["crates"] if safe(crate) and distance(pos, crate["pos"]) < 38]
        crate_target = None
        if not urgent and not danger:
            if drops:
                drop = min(drops, key=lambda item: distance(pos, item["pos"]))
                goal = pos[:2] if distance(pos, drop["pos"]) < drop["radius"] * 0.65 else drop["pos"]
            elif cubes:
                goal = min(cubes, key=lambda item: distance(pos, item["pos"]))["pos"]
            elif pickup and safe(pickup[0]) and "take" not in action:
                goal = pickup[0]["pos"]
            elif crates and not injured and me["cubes"] < self.config["cubes"]["stat_cap"]:
                crate_target = min(crates, key=lambda item: distance(pos, item["pos"]))
                crate_distance = distance(pos, crate_target["pos"])
                if crate_distance > 13:
                    goal = crate_target["pos"]
                elif crate_distance < 8:
                    dx, dy = unit(bearing(crate_target["pos"], pos))
                    goal = (pos[0] + 5 * dx, pos[1] + 5 * dy)
                else:
                    goal = pos[:2]

        # Keep walking when pressured, but bias escape around the ring rather than out of it.
        threat = pressure[0] if pressure else None
        fleeing = attacked or bool(threat and (injured or len(pressure) > 1
                                                or distance(pos, threat["pos"]) < 65))
        away = None
        if fleeing:
            threat_yaw = bearing(pos, threat["pos"]) if threat else self.attacker_yaw
            away = threat_yaw + math.pi
            if distance(pos, goal) < 4 and next_radius > 0:
                dx, dy = unit(away)
                goal = (pos[0] + dx * 28, pos[1] + dy * 28)

        move, jump = self.navigate(me, goal, zone, now, away, urgent)
        self.last_move = move
        action["move"] = list(move)
        action["jump"] = jump
        reserve = me["max_energy"] * (0.14 if urgent else 0.34)
        action["boost"] = bool(
            (jump or (not me["grounded"] and urgent) or
             (me["grounded"] and (urgent or fleeing) and math.hypot(*move) > 0.75))
            and me["energy"] > reserve
        )
        targets = [] if injured and not danger else actors[:4]
        if crate_target:
            targets = [*targets, crate_target]
        attack = self.attack(me, targets, fleeing, urgent)
        if attack:
            action["aim"], action["fire"] = attack
        elif attacked:
            action["aim"] = [self.attacker_yaw, 0]
        elif threat:
            action["aim"] = [bearing(pos, threat["pos"]), 0]
        elif math.hypot(*move) > 0.05:
            action["aim"] = [math.atan2(move[1], move[0]), 0]
        else:
            action["aim"] = [math.remainder(now * 1.1, math.tau), 0]
        if not danger and any(w and w["kind"] == "gun" and 0 < w["ammo"] <= w["stats"]["mag"] / 3
                              for w in me["weapons"]):
            action["reload"] = True
        return action

    def pickup(self, obs):
        me = obs["self"]
        weapons = me["weapons"]
        held = [weapon["kind"] if weapon else None for weapon in weapons]
        ranked = {kind: i for i, kind in enumerate(WEAPONS)}
        eligible = [
            item for item in obs["pickups"]
            if item["kind"] not in held and item["kind"] != "gun"
            and distance(me["pos"], item["pos"]) <= 28
            and abs(me["pos"][2] - item["pos"][2]) <= 4
        ]
        if None in held:
            slot = None
        else:
            slots = [i for i, weapon in enumerate(weapons)
                     if weapon["kind"] != "gun" and not weapon.get("charge")]
            if not slots:
                return None
            slot = min(slots, key=lambda i: ranked[held[i]])
            eligible = [item for item in eligible if ranked[item["kind"]] > ranked[held[slot]]]
        if eligible:
            return min(eligible, key=lambda item: (
                max(0, distance(me["pos"], item["pos"]) - self.config["loot"]["pickup_radius"])
                - 4 * ranked[item["kind"]]
            )), slot
        return None

    def navigate(self, me, goal, zone, now, away, urgent):
        pos, stats = me["pos"], me["stats"]
        dist = distance(pos, goal)
        # Detect sustained lack of progress and keep turning the same way around the obstacle.
        if self.watch_position is None or now - self.watch_time >= 0.8:
            if (self.watch_position is not None and dist > 5 and math.hypot(*self.last_move) > 0.5
                    and distance(pos, self.watch_position) < 1.5):
                self.detour_until = now + 2.5
                if self.detour_heading is None:
                    self.detour_heading = bearing(pos, goal) + self.detour_side * math.pi / 2
            self.watch_position, self.watch_time = pos[:2], now
        if now >= self.detour_until:
            self.detour_heading = None
        if dist < 0.65:
            return (0, 0), False

        direct = bearing(pos, goal)
        probe = min(11.0, max(2.5, dist))
        preferred = self.detour_heading if self.detour_heading is not None else direct
        best = None
        for offset in (0, 25, -25, 50, -50, 75, -75, 105, -105, 145, -145, 180):
            yaw = preferred + math.radians(offset)
            dx, dy = unit(yaw)
            clear = self.arena.travel(pos, (dx, dy), probe, stats)
            if clear < min(2.5, probe):
                continue
            end = (pos[0] + dx * clear, pos[1] + dy * clear)
            progress = dist - distance(end, goal)
            ring_overflow = max(0, distance(end, zone["center"]) - max(0, zone["radius"] - 3))
            next_overflow = max(0, distance(end, zone["next_center"]) - zone["next_radius"])
            score = (progress * (0.6 if self.detour_heading is not None else 1)
                     + 0.4 * clear - 2.4 * ring_overflow - 0.45 * next_overflow)
            score += 1.8 * (dx * self.last_move[0] + dy * self.last_move[1])
            if self.detour_heading is not None:
                score += 5 * math.cos(delta(yaw, preferred))
            if away is not None:
                score += (0.3 if urgent else 1.0) * clear * math.cos(delta(yaw, away))
            if best is None or score > best[0]:
                best = score, (dx, dy), clear
        if best:
            _, direction, clear = best
            speed = min(1.0, dist / 3, clear / 4)
            return tuple(value * speed for value in direction), False
        return unit(preferred), me["grounded"] and me["energy"] > 0.35 * me["max_energy"]

    def attack(self, me, targets, fleeing, urgent):
        pos, stats = me["pos"], me["stats"]
        best = None
        for target in targets:
            kind = target.get("kind", "crate")
            geometry = {"mech": self.config["mech"], "npc": self.config["npc"],
                        "crate": self.config["crates"]}[kind]
            size = 1 + target.get("cubes", 0) * self.config["cubes"]["size_per_cube"]
            height = geometry["height"] * size
            radius = geometry["radius"] * size
            center = [*target["pos"][:2], target["pos"][2] + height * 0.55]
            horizontal = distance(pos, center)
            if horizontal < 0.1:
                continue
            for slot, weapon in enumerate(me["weapons"]):
                if not weapon:
                    continue
                name, w = weapon["kind"], weapon["stats"]
                charge = weapon.get("charge", 0)
                if ((weapon["cooldown"] > 0.10 and not charge)
                        or (name == "gun" and (not weapon["ammo"] or weapon["reload"] > 0.05))
                        or (name == "laser" and (fleeing or urgent) and not charge)
                        or (name == "lightning" and me["energy"] < w["energy"] + (26 if fleeing else 12))):
                    continue
                muzzle = pos[2] + stats["muzzle_height"] + (2.5 if name == "mortar" else 0)
                origin = (*pos[:2], muzzle)
                speed = w.get("speed", 0)
                flight = max(0, horizontal - stats["radius"] - 0.5) / speed if speed else 0
                vel = target.get("vel", (0, 0, 0))
                impact = tuple(c + v * flight for c, v in zip(center, vel))
                travel = distance(origin, impact)
                vertical = impact[2] - muzzle
                gravity = 0
                if name == "mortar":
                    if travel < w["splash_radius"] + 5 or flight > 1.7:
                        continue
                    travel = max(1, travel - stats["radius"] - 0.5)
                    gravity = w["gravity"]
                    discriminant = speed**4 - gravity * (gravity * travel**2 + 2 * vertical * speed**2)
                    if discriminant < 0:
                        continue
                    pitch = math.atan((speed**2 - math.sqrt(discriminant)) / (gravity * travel))
                    flight = travel / (speed * math.cos(pitch))
                else:
                    if math.dist(origin, impact) > w["range"] - 0.5:
                        continue
                    pitch = math.atan2(vertical, travel)
                if not stats["min_pitch"] <= pitch <= stats["max_pitch"]:
                    continue
                yaw = bearing(pos, impact)
                yaw_error = abs(delta(yaw, me["aim"][0]))
                pitch_error = abs(pitch - me["aim"][1])
                start_offset = stats["radius"] + 0.5
                # Test the whole line, including the barrel path near walls.
                if not self.arena.clear(origin, impact, gravity, flight):
                    continue
                if name == "lightning":
                    aligned = yaw_error < math.radians(w["cone_deg"]) * 0.45
                    trigger = aligned
                else:
                    tolerance = math.atan2(radius * (0.9 if name == "mortar" else 0.75),
                                           max(1, travel - start_offset))
                    aligned = yaw_error < tolerance and pitch_error < max(tolerance, math.atan2(height * 0.35, travel))
                    if name == "laser":
                        if charge:
                            trigger = not (fleeing or urgent) and (charge < w["charge"] or not aligned)
                        else:
                            trigger = yaw_error < 0.3
                    else:
                        trigger = aligned

                score = {"gun": 32, "mortar": 51, "laser": 53, "lightning": 58}[name]
                score += {"crate": -20, "npc": -12, "mech": 0}[kind]
                score += (16 if aligned else 0) - 22 * yaw_error - 0.14 * horizontal
                score += 75 if charge else 0
                if target.get("hp", 1000) < w["damage"]:
                    score += 10
                if best is None or score > best[0]:
                    best = score, [yaw, pitch], [i == slot and trigger for i in range(2)]
        if best:
            return best[1], best[2]
        return None


if __name__ == "__main__":
    mf.run(Bot)
