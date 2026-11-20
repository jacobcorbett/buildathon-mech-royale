"""Incremental ground routes around arena obstacles."""
from collections import deque
from functools import lru_cache
from heapq import heappop, heappush
import math


CARDINAL = ((1, 0), (-1, 0), (0, 1), (0, -1))
DIAGONAL = ((1, 1), (1, -1), (-1, 1), (-1, -1))
NEIGHBOURS = CARDINAL + DIAGONAL


class GroundRouter:
    spacing = 4
    expansions_per_tick = 96
    search_limit = 4000

    def __init__(self, arena):
        self.arena = arena
        self.waypoints = deque()
        self.goal = None
        self.profile = None
        self.search = None
        self.retry_at = 0
        self.walkable = lru_cache(maxsize=16000)(self.walkable)

    def cell(self, point):
        return tuple(round(v / self.spacing) for v in point[:2])

    def point(self, cell):
        return tuple(v * self.spacing for v in cell)

    def walkable(self, cell, profile):
        x, y = self.point(cell)
        radius, height, step = profile
        floor = self.arena.ground(x, y)
        if not self.arena.within_bounds(x, y, radius):
            return False
        for dx, dy in ((0, 0), *NEIGHBOURS):
            scale = radius * (math.sqrt(0.5) if dx and dy else 1)
            sx, sy = x + dx * scale, y + dy * scale
            if any(top > floor + step and base < floor + height
                   for base, top in self.arena.spans(sx, sy)):
                return False
        return True

    def snap(self, point, profile, limit):
        cx, cy = self.cell(point)
        if self.walkable((cx, cy), profile):
            return cx, cy
        for radius in range(1, limit + 1):
            ring = (
                (cx + dx, cy + dy)
                for dx in range(-radius, radius + 1)
                for dy in range(-radius, radius + 1)
                if max(abs(dx), abs(dy)) == radius
            )
            nearest = min((cell for cell in ring if self.walkable(cell, profile)),
                          key=lambda cell: math.dist(self.point(cell), point[:2]), default=None)
            if nearest is not None:
                return nearest
        return None

    def solve(self, start, end, profile, zone, stats):
        fringe = [(math.dist(start, end), 0, start)]
        distances = {start: 0}
        parents = {}
        slope = math.tan(math.radians(stats["max_slope_deg"]))
        center = zone["center"]
        radius = zone["radius"] - 3
        covered = 0
        while fringe and covered < self.search_limit:
            _, cost, cell = heappop(fringe)
            if cost != distances[cell]:
                continue
            if cell == end:
                path = deque()
                while cell != start:
                    path.appendleft(self.point(cell))
                    cell = parents[cell]
                return path
            x, y = self.point(cell)
            floor = self.arena.ground(x, y)
            for dx, dy in NEIGHBOURS:
                nx, ny = other = (cell[0] + dx, cell[1] + dy)
                if not self.walkable(other, profile):
                    continue
                if dx and dy and (not self.walkable((nx, cell[1]), profile)
                                  or not self.walkable((cell[0], ny), profile)):
                    continue
                px, py = self.point(other)
                length = math.sqrt(2) if dx and dy else 1
                rise = self.arena.ground(px, py) - floor
                if abs(rise) > max(profile[2], length * self.spacing * slope):
                    continue
                overflow = max(0, math.dist((px, py), center) - radius)
                other_cost = cost + length + 0.1 * min(40, overflow)
                if other_cost < distances.get(other, math.inf):
                    distances[other] = other_cost
                    parents[other] = cell
                    heappush(fringe, (other_cost + math.dist(other, end), other_cost, other))
            covered += 1
            if covered % self.expansions_per_tick == 0:
                yield
        return deque()

    def waypoint(self, pos, goal, stats, zone, now):
        goal = tuple(goal[:2])
        if math.dist(pos[:2], goal) < 3:
            self.goal = None
            return goal
        if abs(pos[2] - self.arena.ground(*pos[:2])) > 2.5:
            self.goal = None
            return goal

        # Round up body dimensions so cube growth never makes a cached cell too optimistic.
        profile = (math.ceil(stats["radius"] * 2) / 2 + 0.5, math.ceil(stats["height"]),
                   stats["step_up"])
        if self.goal is None or math.dist(goal, self.goal) > 9 or profile != self.profile:
            self.goal, self.profile = goal, profile
            self.waypoints.clear()
            self.search = None
            self.retry_at = now

        dist = math.dist(pos[:2], goal)
        direction = ((goal[0] - pos[0]) / dist, (goal[1] - pos[1]) / dist)
        lookahead = min(dist, 26)
        direct_enough = dist <= lookahead or (not self.waypoints and self.search is None)
        if direct_enough and self.arena.travel(pos, direction, lookahead, stats) >= lookahead:
            self.waypoints.clear()
            self.search = None
            return goal

        if self.search is None and not self.waypoints and now >= self.retry_at:
            start = self.snap(pos, profile, 2)
            end = self.snap(goal, profile, 5)
            if start is not None and end is not None and start != end:
                self.search = self.solve(start, end, profile, zone, stats)
            self.retry_at = now + 1.5
        if self.search is not None:
            try:
                next(self.search)
            except StopIteration as result:
                self.waypoints = result.value
                self.search = None

        while self.waypoints and math.dist(pos[:2], self.waypoints[0]) < 2.5:
            if len(self.waypoints) == 1 and math.dist(self.waypoints[0], goal) > self.spacing:
                # The requested point is blocked; hold the nearest reachable ground.
                return self.waypoints[0]
            self.waypoints.popleft()
        if not self.waypoints:
            return goal

        # Skip nearby waypoints only when the body's whole probe fits the shortcut.
        for i in range(min(5, len(self.waypoints)) - 1, 0, -1):
            point = self.waypoints[i]
            dist = math.dist(pos[:2], point)
            if dist < 0.01:
                return point
            direction = ((point[0] - pos[0]) / dist, (point[1] - pos[1]) / dist)
            if dist <= 18 and self.arena.travel(pos, direction, dist, stats) >= dist:
                for _ in range(i):
                    self.waypoints.popleft()
                break
        return self.waypoints[0]
