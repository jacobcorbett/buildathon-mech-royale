"""Read the match map for short movement probes and approximate shot clearance."""
from array import array
import json
import math
from pathlib import Path
import sys


class Arena:
    cell = 2

    def __init__(self, directory):
        directory = Path(directory) / "v2"
        world = json.loads((directory / "world.json").read_text())
        self.bounds = world["bounds"]
        self.grid = world["terrain"]
        self.terrain = array("h", (directory / "terrain.i16").read_bytes())
        if sys.byteorder != "little":
            self.terrain.byteswap()

        self.solids = {}
        for prism in world["prisms"]:
            rings = [prism["ring"], *prism["holes"]]
            edges = []
            for ring in rings:
                points = list(zip(ring[::2], ring[1::2]))
                edges.extend(zip(points, points[1:] + points[:1]))
            ys = prism["ring"][1::2]
            heights = (prism["base"], prism["top"])
            for row in range(math.floor(min(ys) / self.cell), math.ceil(max(ys) / self.cell)):
                y = (row + 0.5) * self.cell
                crossings = sorted(
                    x1 + (x2 - x1) * (y - y1) / (y2 - y1)
                    for (x1, y1), (x2, y2) in edges
                    if (y1 > y) != (y2 > y)
                )
                for left, right in zip(crossings[::2], crossings[1::2]):
                    for col in range(math.ceil(left / self.cell - 0.5),
                                     math.ceil(right / self.cell - 0.5)):
                        self.solids.setdefault((col, row), []).append(heights)

    def ground(self, x, y):
        g = self.grid
        fx = min(g["nx"] - 1.00001, max(0, (x - g["minX"]) / g["step"]))
        fy = min(g["ny"] - 1.00001, max(0, (y - g["minY"]) / g["step"]))
        col, row = int(fx), int(fy)
        dx, dy = fx - col, fy - row
        i = row * g["nx"] + col
        j = i + g["nx"]
        return g["scale"] * (
            (1 - dy) * ((1 - dx) * self.terrain[i] + dx * self.terrain[i + 1])
            + dy * ((1 - dx) * self.terrain[j] + dx * self.terrain[j + 1])
        )

    def spans(self, x, y):
        return self.solids.get((math.floor(x / self.cell), math.floor(y / self.cell)), ())

    def within_bounds(self, x, y, radius):
        b = self.bounds
        return (b["minX"] + radius <= x <= b["maxX"] - radius
                and b["minY"] + radius <= y <= b["maxY"] - radius)

    def travel(self, pos, direction, distance, stats):
        """Return unobstructed metres along a short probe, allowing steps and bridges."""
        x, y, z = pos
        dx, dy = direction
        radius = stats["radius"]
        step = stats["step_up"]
        max_rise = max(step, math.tan(math.radians(stats["max_slope_deg"])))
        for along in range(1, math.ceil(distance) + 1):
            forward = min(distance, along)
            px, py = x + dx * forward, y + dy * forward
            if not self.within_bounds(px, py, radius):
                return max(0.0, forward - 1)
            floor = self.ground(px, py)
            if floor > z + max_rise:
                return max(0.0, forward - 1)
            for offset in (0, radius, -radius):
                sx, sy = px - dy * offset, py + dx * offset
                for base, top in self.spans(sx, sy):
                    if top > z + step + 0.25 and base < z + stats["height"]:
                        return max(0.0, forward - 1)
            # Update the probe's foot height on ordinary slopes, without dropping off roofs.
            if floor >= z - 2:
                z = floor
        return distance

    def clear(self, start, end, gravity=0, flight=0):
        dx, dy, dz = (b - a for a, b in zip(start, end))
        steps = max(2, math.ceil(math.dist(start, end) / 2), math.ceil(flight * 16))
        for i in range(1, steps):
            t = i / steps
            x, y = start[0] + dx * t, start[1] + dy * t
            z = start[2] + dz * t + 0.5 * gravity * flight**2 * t * (1 - t)
            if z < self.ground(x, y) or any(base < z < top for base, top in self.spans(x, y)):
                return False
        return True
