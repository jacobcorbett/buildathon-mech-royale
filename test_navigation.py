"""Ground routing regressions with deterministic, flat test arenas."""
import math
import unittest

from arena import Arena
from bot import Bot
from navigation import GroundRouter


STATS = {"radius": 2.5, "height": 9, "step_up": 1.2, "max_slope_deg": 50}
ZONE = {"center": [0, 0], "radius": 120, "next_center": [0, 0], "next_radius": 80}


class FlatArena(Arena):
    def __init__(self, walls):
        self.walls = walls
        self.bounds = {"minX": -90, "maxX": 90, "minY": -90, "maxY": 90}

    def ground(self, x, y):
        return 0

    def spans(self, x, y):
        return [(base, 20) for x1, x2, y1, y2, base in self.walls
                if x1 <= x <= x2 and y1 <= y <= y2]


class NavigationTest(unittest.TestCase):
    def test_crosses_u_shaped_obstacle(self):
        # Start inside the U. The opening faces away from the destination.
        arena = FlatArena([(4, 12, -24, 24, 0), (-32, 12, -24, -16, 0),
                           (-32, 12, 16, 24, 0)])
        bot = Bot.__new__(Bot)
        bot.arena = arena
        bot.last_move = (0, 0)
        bot.watch_position, bot.watch_time = None, 0
        bot.detour_until, bot.detour_heading, bot.detour_side = -1, None, 1
        router = GroundRouter(arena)
        pos, goal = [-12, 0, 0], (36, 0)
        minimum_x = pos[0]
        # Kinematic fixture at 10 m/s. The real server owns acceleration and collision.
        for tick in range(450):
            now = tick / 10
            waypoint = router.waypoint(pos, goal, STATS, ZONE, now)
            me = {"pos": pos, "stats": STATS, "grounded": True, "energy": 0, "max_energy": 100}
            move, _ = bot.navigate(me, waypoint, ZONE, now, None, False, waypoint != goal)
            bot.last_move = move
            length = math.hypot(*move)
            if length and arena.travel(pos, tuple(v / length for v in move), length, STATS) == length:
                pos[0] += move[0]
                pos[1] += move[1]
            minimum_x = min(minimum_x, pos[0])
            if math.dist(pos[:2], goal) < 2:
                break
        self.assertLess(minimum_x, -34)
        self.assertLess(math.dist(pos[:2], goal), 2)

    def test_clear_path_under_bridge(self):
        arena = FlatArena([(4, 12, -60, 60, 12)])
        router = GroundRouter(arena)
        self.assertEqual(router.waypoint((0, 0, 0), (24, 0), STATS, ZONE, 0), (24, 0))
        self.assertIsNone(router.search)

    def test_airborne_probe_keeps_destination(self):
        arena = FlatArena([(4, 12, -60, 60, 0)])
        router = GroundRouter(arena)
        self.assertEqual(router.waypoint((0, 0, 25), (24, 0), STATS, ZONE, 0), (24, 0))
        self.assertIsNone(router.search)

    def test_cube_growth_invalidates_small_clearance(self):
        arena = FlatArena([(4, 12, -60, -4, 0), (4, 12, 4, 60, 0)])
        router = GroundRouter(arena)
        self.assertTrue(router.walkable((2, 0), (2.5, 9, 1.2)))
        self.assertFalse(router.walkable((2, 0), (4, 12, 1.2)))


if __name__ == "__main__":
    unittest.main()
