"""B2's f64 reference of the body model (plan 2.1-2.4) on the analytic cases of plan 3.1."""

import math
import unittest

import anim_body_support as support
from anim_body_model_support import BITS, F32, f32

import anim_body_model as model  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_tolerance as tolerance  # noqa: E402
import anim_fixture_body as body  # noqa: E402
from anim_reference import angle, euler, multiply  # noqa: E402

# A five-bone rig in its hips frame: the four frame bones straight under the hips, F(T) the identity.
LOCAL = {"Hips": (0., 0., 0.), "LeftUpperLeg": (-.1, -.1, 0.), "RightUpperLeg": (.1, -.1, 0.),
         "LeftUpperArm": (-.2, .5, 0.), "RightUpperArm": (.2, .5, 0.)}
MASSES = dict.fromkeys(LOCAL, .2)
LINKS = [("Hips", name) for name in LOCAL if name != "Hips"]
HALF = {f"{i}>{j}": .5 for i, j in LINKS}
# c_h = .2 (0, .8, 0) + .5 .2 (0, .8, 0).
C_H = (0., .24, 0.)
IDENTITY = tolerance.Frame((0., 0., 0.), (0., 0., 0., 1.), 1., 0., 0)
SCALE = 2.
TARGET = (.1, .62, -.05)


def yaw(degrees):
    return euler((0, degrees, 0), 4)


class CentreTests(unittest.TestCase):
    def assertClose(self, got, want, places=14):
        for a, b in zip(got, want, strict=True):
            self.assertAlmostEqual(a, b, places=places)

    def test_a_single_segment(self):
        points = {"Hips": (0., 1., 0.), "Spine": (0., 1.5, 0.)}
        masses = {"Hips": .6, "Spine": .4}
        # .6 (0, 1, 0) + .4 (0, 1.5, 0) + .25 .6 (0, .5, 0).
        self.assertClose(model.centre(points, masses, [("Hips", "Spine")], {"Hips>Spine": .25}), (0., 1.275, 0.))
        self.assertClose(model.columns(points, masses, [("Hips", "Spine")])["Hips>Spine"], (0., .3, 0.))
        # Without a coefficient the mass sits at the bone's origin.
        self.assertClose(model.centre(points, masses, [("Hips", "Spine")], {}), (0., 1.2, 0.))

    def test_an_absent_optional_bone_folds_and_links_past(self):
        mapped = set(support.PARENT) - {"UpperChest"}
        folded = tolerance.folded_masses(support.MASSES, support.PARENT, mapped)
        total = sum(support.MASSES.values())
        self.assertAlmostEqual(folded["Chest"], (support.MASSES["Chest"] + support.MASSES["UpperChest"]) / total)
        links = tolerance.edges(support.PARENT, mapped)
        self.assertIn(("Chest", "Neck"), links)
        self.assertNotIn("UpperChest", {bone for link in links for bone in link})
        # The centre then weights the chest by the folded mass and its edge to the neck.
        points = {name: (0., float(i), 0.) for i, name in enumerate(tolerance.ordered(mapped))}
        want = (sum(folded[n] * points[n][1] for n in mapped)
                + .5 * folded["Chest"] * (points["Neck"][1] - points["Chest"][1]))
        got = model.centre(points, folded, [("Chest", "Neck")], {"Chest>Neck": .5})[1]
        self.assertAlmostEqual(got, want, places=13)

    def test_the_quaternion_of_a_matrix_on_every_branch(self):
        # Shepperd's method from each largest component, near the identity and near half turns about each axis,
        # where a component from the square root of a cancelled difference would lose half its digits.
        for turn in ((1e-4, -2e-4, 3e-4), (179.99, 1e-3, 0), (1e-3, 179.99, 0), (0, 1e-3, 179.99), (30, -45, 60)):
            q = euler(turn, 4)
            q = q if q[3] >= 0 else tuple(-v for v in q)
            got = model.quaternion(tolerance.quaternion_matrix(q))
            self.assertLess(max(abs(a - b) for a, b in zip(got, q)), 4e-16, turn)

    def test_the_frame_of_the_rest_pose_and_a_turned_one(self):
        self.assertClose([v for row in tolerance.body_frame(LOCAL) for v in row], [1, 0, 0, 0, 1, 0, 0, 0, 1])
        turn = euler((10, -35, 20), 4)
        turned = {name: tolerance.rotate(turn, p) for name, p in LOCAL.items()}
        self.assertLess(angle(model.body_rotation(turned, tolerance.body_frame(LOCAL)), turn), 1e-14)


class ScopeTests(unittest.TestCase):
    def test_the_topology_scope(self):
        refusals = {rig: None for rig in body.PROBE_RIGS}
        for rig in body.PROBE_RIGS:
            try:
                model.topology_scope(body.rig_bones(rig))
            except model.Refused as exc:
                refusals[rig] = str(exc)
        self.assertEqual({rig: reason for rig, reason in refusals.items() if reason},
                         {"no-chest": "topology: Chest absent", "no-neck": "topology: Neck absent",
                          "no-left-shoulder": "topology: exactly one Shoulder",
                          "no-right-shoulder": "topology: exactly one Shoulder"})
        # The admitted patterns are exactly the in-scope training rigs'; any other refuses, such as one eye.
        self.assertEqual(model.RECORDED_ABSENT, {tolerance.BODY_SLOTS - body.rig_bones(rig)
                                                 for rig in body.PROBE_RIGS.keys() - {body.HOLDOUT}
                                                 if not refusals[rig]})
        with self.assertRaisesRegex(model.Refused, r"unrecorded pattern \(absent \['Jaw', 'RightEye'\]\)"):
            model.topology_scope(body.rig_bones("compact") | {"LeftEye"})

    def test_the_twist_scope(self):
        def twisted(degrees):
            turn = yaw(degrees)
            return {name: tolerance.rotate(turn, p) if "Arm" in name else p for name, p in LOCAL.items()}

        rest = tolerance.body_frame(LOCAL)
        self.assertAlmostEqual(math.degrees(model.shoulder_hip_twist(twisted(4))), 4., places=12)
        model.body_rotation(twisted(4), rest)
        for refused in (lambda: model.body_rotation(twisted(-5), rest),
                        lambda: model.solve_hips(TARGET, (0., 0., 0., 1.), SCALE, twisted(5), MASSES, LINKS, HALF,
                                                 rest, IDENTITY, 1.)):
            with self.assertRaisesRegex(model.Refused, "shoulder-hip twist 5.000 deg"):
                refused()
        # A lateral along up has no twist about it: undefined, refused rather than read as zero.
        along = dict(LOCAL)
        mid = tuple((a + b) / 2 for a, b in zip(LOCAL["LeftUpperArm"], LOCAL["RightUpperArm"]))
        up = tuple(a - (b + c) / 2 for a, b, c in zip(mid, LOCAL["LeftUpperLeg"], LOCAL["RightUpperLeg"]))
        along["LeftUpperArm"] = tuple(m - u / 4 for m, u in zip(mid, up))
        along["RightUpperArm"] = tuple(m + u / 4 for m, u in zip(mid, up))
        with self.assertRaisesRegex(model.Refused, "undefined shoulder-hip twist"):
            model.shoulder_hip_twist(along)


class SolveTests(unittest.TestCase):
    def solve(self, rotation=(0., 0., 0., 1.), parent=IDENTITY, hips_scale=1., local=LOCAL):
        return model.solve_hips(TARGET, rotation, SCALE, local, MASSES, LINKS, HALF, tolerance.body_frame(LOCAL),
                                parent, hips_scale)

    def assertClose(self, got, want, places=14):
        for a, b in zip(got, want, strict=True):
            self.assertAlmostEqual(a, b, places=places)

    def test_the_rest_pose_on_an_identity_root(self):
        # P_A = h T_A - c_h.
        t, q = self.solve()
        self.assertClose(t, (.2, 1.0, -.1))
        self.assertLess(angle(q, (0, 0, 0, 1)), 1e-15)

    def test_a_rotated_and_translated_root(self):
        # t = R_y(-90) ((.2, 1, -.1) - (1, 2, 3)) = (3.1, -1, -.8), q = R_y(-90).
        t, q = self.solve(parent=tolerance.Frame((1., 2., 3.), yaw(90), 1., 0., 1))
        self.assertClose(t, (3.1, -1., -.8))
        self.assertLess(angle(q, yaw(-90)), 1e-15)

    def test_a_uniform_intermediate_scale(self):
        # k = 2: P_A = (.2, 1.24 - .48, -.1), t = P_A / 2.
        t, _ = self.solve(parent=tolerance.Frame((0., 0., 0.), (0., 0., 0., 1.), 2., 0., 1))
        self.assertClose(t, (.1, .38, -.05))

    def test_a_non_unit_hips_scale(self):
        # k = s_h = 1.5: P_A = (.2, 1.24 - .36, -.1).
        t, _ = self.solve(hips_scale=1.5)
        self.assertClose(t, (.2, .88, -.1))

    def test_a_noncommuting_pose(self):
        # The pose turns its frame by R_z(30) and the body asks for R_x(40): H_A = R_x(40) R_z(-30), and
        # P_A = h T_A - H_A R_z(30) c_h = h T_A - R_x(40) c_h.
        spin, tilt = euler((0, 0, 30), 4), euler((40, 0, 0), 4)
        local = {name: tolerance.rotate(spin, p) for name, p in LOCAL.items()}
        t, q = self.solve(rotation=tilt, local=local)
        want = multiply(tilt, euler((0, 0, -30), 4))
        self.assertLess(angle(q, want), 1e-14)
        self.assertGreater(angle(q, multiply(euler((0, 0, -30), 4), tilt)), .1)
        c, s = math.cos(math.radians(40)), math.sin(math.radians(40))
        self.assertClose(t, (.2, 1.24 - .24 * c, -.1 - .24 * s))
        # Placed by (t, q), the pose's centre is the target and its frame the request.
        placed = model.compose(local, q, t)
        self.assertClose(model.centre(placed, MASSES, LINKS, HALF), tuple(SCALE * v for v in TARGET))
        self.assertLess(angle(model.body_rotation(placed, tolerance.body_frame(LOCAL)), tilt), 1e-14)

    def test_the_solve_within_its_frozen_f64_bound(self):
        # The five-bone rig as recorded rows under a turned, scaled hips parent: the model's own error bound.
        names = list(LOCAL)
        nodes = [dict(address="0"), dict(address="0/0"), dict(address="0/0/0")]
        nodes += [dict(address=f"0/0/0/{k}") for k in range(len(names) - 1)]

        def row(t=(0., 0., 0.), q=(0., 0., 0., 1.), s=1.):
            return dict(translation=[BITS(v) for v in t], rotation=[BITS(v) for v in q], scale=[BITS(s)] * 3)

        parent_rotation = tuple(f32(v) for v in yaw(90))
        transforms = [row(), row((1, 2, 3), parent_rotation, 2.), row((.5, .25, 0)),
                      *(row(LOCAL[name]) for name in names[1:])]
        bones_rows = [dict(name=name, node=2 + k) for k, name in enumerate(names)]
        local, differences = tolerance.body_states(nodes, transforms, F32, bones_rows, LINKS, "0/0/0", tolerance.U64)
        parent = tolerance.tracked(nodes, transforms, F32, "0", tolerance.U64)["0/0"]
        frame = tolerance.body_frame({name: bone.position for name, bone in local.items()})
        bound = tolerance.evaluation_error(local, differences, local, MASSES, LINKS, SCALE, TARGET, (0, 0, 0, 1),
                                           frame, parent, 1.)
        t, _ = model.solve_hips(TARGET, (0, 0, 0, 1), SCALE, {n: b.position for n, b in local.items()}, MASSES,
                                LINKS, HALF, frame, parent, 1.)
        # k = sigma_p = 2, H_A = I: t = R_p^-1 (h T_A - 2 c_h - t_p) / 2 with R_p the f32 yaw.
        p_a = (.2, .76, -.1)
        want = tuple(v / 2 for v in tolerance.rotate(tolerance.conjugate(parent_rotation),
                                                     tolerance.sub(p_a, (1, 2, 3))))
        self.assertLess(max(abs(a - b) for a, b in zip(t, want)), bound)
        self.assertLess(bound, 1e-6)


class ProjectionTests(unittest.TestCase):
    BODY = ((1., .9, 2.), multiply(yaw(30), euler((10, 0, 0), 4)))
    START = ((.5, 1., .2), yaw(-20))
    ORIGINAL = ((.3, .05, .4), yaw(15))

    @staticmethod
    def serialized(bake=(0, 0, 0), keep=(0, 0, 0), feet=False, start=True):
        fields = dict(zip(("m_LoopBlendOrientation", "m_LoopBlendPositionY", "m_LoopBlendPositionXZ"), map(bool, bake)))
        fields |= dict(zip(("m_KeepOriginalOrientation", "m_KeepOriginalPositionY", "m_KeepOriginalPositionXZ"), keep))
        return fields | dict(m_HeightFromFeet=feet, m_StartAtOrigin=start)

    def project(self, **flags):
        return model.project(model.cells(self.serialized(**flags)), self.BODY, self.START, self.ORIGINAL, .125, 10.)

    def assertRoot(self, root, position, degrees):
        for a, b in zip(root[0], position, strict=True):
            self.assertAlmostEqual(a, b, places=14)
        self.assertLess(angle(root[1], yaw(degrees)), 1e-14)

    def test_each_cell(self):
        # Body: its XZ, height plus the level, its twist about up plus the offset.
        self.assertRoot(self.project(), (1., 1.025, 2.), 40)
        # Bake: the start values, with no level or offset.
        self.assertRoot(self.project(bake=(1, 1, 1)), (.5, 1., .2), -20)
        # Keep original: the motion root, with the level and the offset.
        self.assertRoot(self.project(keep=(1, 1, 1)), (.3, .175, .4), 25)
        # Mixed, one cell each: yaw baked, height original, XZ from the body.
        self.assertRoot(self.project(bake=(1, 0, 0), keep=(0, 1, 0)), (1., .175, 2.), -20)
        self.assertEqual(model.cells(self.serialized(bake=(1, 0, 0), keep=(1, 1, 0))),
                         dict(yaw="bake", height="original", xz="body"))

    def test_refused_and_undecided_cells(self):
        with self.assertRaises(model.Undecided):
            self.project(feet=True)
        with self.assertRaises(model.Refused):
            model.cells(self.serialized(start=False))
        with self.assertRaises(model.Refused):
            model.project(model.cells(self.serialized(keep=(0, 0, 1))), self.BODY, self.START, None)
        with self.assertRaises(ValueError):
            model.cells(self.serialized() | dict(m_HeightFromFeet=2))
        # Height from feet under a baked or original height is not the feet rule.
        self.assertEqual(model.cells(self.serialized(bake=(0, 1, 0), feet=True))["height"], "bake")

    def test_the_body_in_the_animator_frame(self):
        root = self.project()
        position, rotation = model.animator_body(self.BODY, root, 1.5)
        # T - r = (0, -.125, 0), so the yaw does not enter; B_A = R_y(-40) R_y(30) R_x(10).
        for a, b in zip(position, (0., -.1875, 0.)):
            self.assertAlmostEqual(a, b, places=14)
        self.assertLess(angle(rotation, multiply(yaw(-10), euler((10, 0, 0), 4))), 1e-14)

    def test_a_shared_rigid_move_cancels(self):
        # Moving the body, its start and the motion root together leaves (T_A, B_A) unchanged in every cell.
        move, offset = yaw(50), (.6, 0., -.4)

        def moved(pose):
            return tuple(a + b for a, b in zip(tolerance.rotate(move, pose[0]), offset)), multiply(move, pose[1])

        for bake, keep in (((0, 0, 0), (0, 0, 0)), ((1, 1, 1), (0, 0, 0)), ((0, 0, 0), (1, 1, 1))):
            rules = model.cells(self.serialized(bake=bake, keep=keep))
            before = model.animator_body(self.BODY, model.project(rules, self.BODY, self.START, self.ORIGINAL), 1.)
            after = model.animator_body(moved(self.BODY), model.project(
                rules, moved(self.BODY), moved(self.START), moved(self.ORIGINAL)), 1.)
            self.assertLess(math.dist(before[0], after[0]), 1e-14)
            self.assertLess(angle(before[1], after[1]), 1e-14)

    def test_the_projection_bounds(self):
        root = self.project()
        position, turn = model.projection_error(self.BODY, root, 1.5, model.yaw_error(self.BODY[1]))
        self.assertTrue(0 < position < 1e-14 and 0 < turn < 1e-14)
        # A half turn about a horizontal axis has no twist about up.
        flipped = (1., 0., 0., 0.)
        self.assertEqual(model.yaw_error(flipped), math.inf)
        with self.assertRaises(model.Refused):
            model.yaw(flipped)

    def test_a_yaw_is_the_twist_about_up(self):
        # A tilted body: its twist about up, not its forward's heading, as the training fixtures show.
        q = multiply(yaw(25), euler((10, 0, 15), 4))
        f = tolerance.rotate(q, (0., 0., 1.))
        self.assertAlmostEqual(model.yaw(q), 2 * math.atan2(q[1], q[3]), places=15)
        self.assertGreater(abs(model.yaw(q) - math.atan2(f[0], f[2])), math.radians(.1))
        # Turned on the vertical, the twist passes through; a forward pointing up keeps its twist.
        self.assertAlmostEqual(model.yaw(multiply(yaw(40), q)), model.yaw(q) + math.radians(40), places=14)
        self.assertAlmostEqual(model.yaw(euler((-90, 0, 0), 4)), 0., places=15)
        self.assertAlmostEqual(model.turn_angle(2 * math.sin(.25)), .5, places=14)
        # A turn of 2 or more bounds nothing: no comparison against it passes.
        self.assertTrue(math.isnan(model.turn_angle(3.)) and math.isnan(model.turn_angle(math.inf)))
        self.assertFalse(0. <= model.turn_angle(2.))

    def test_authored_curves(self):
        def keys(*rows):
            return [dict(time_f32=BITS(t), value_f32=BITS(v), in_tangent_f32=BITS(m), out_tangent_f32=BITS(m),
                         weighted_mode="None") for t, v, m in rows]

        self.assertEqual(model.curve(keys((0, 0, 0), (1, 1, 0)), .5)[0], .5)
        self.assertAlmostEqual(model.curve(keys((0, 0, 1), (1, 1, 1)), .25)[0], .25, places=15)
        self.assertEqual(model.curve(keys((0, 2, 0), (1, 3, 0)), 1.5)[0], 3)
        curves = {f"RootQ.{a}": dict(keys=keys((0, v, 0), (1, v, 0))) for a, v in zip("xyzw", (0, 0, 0, 2))}
        q, error = model.sample_curves(curves, "RootQ", .5)
        self.assertEqual(q, (0, 0, 0, 1))
        self.assertGreater(error, 0)
        self.assertEqual(model.sample_curves(curves, "MotionT", .5), (None, 0.))
        weighted = keys((0, 0, 0), (1, 1, 0))
        weighted[0]["weighted_mode"] = "Both"
        with self.assertRaises(model.Refused):
            model.curve(weighted, .5)


if __name__ == "__main__":
    unittest.main()
