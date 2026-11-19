"""B1's operation-specific error bounds for the humanoid body model, by hand and against emulated f32."""

import json
import math
import random
import struct
import unittest

import anim_body_support as support
from anim_body_support import body_doc
from anim_fixture_support import RUN

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_tolerance as tolerance  # noqa: E402
import anim_fixture_body as body  # noqa: E402
from anim_reference import euler  # noqa: E402
import anim_replay as replay  # noqa: E402

G = tolerance.g
U = 2. ** -24
F32 = replay.f32_from_bits


def gamma(n, unit=U):
    return n * unit / (1 - n * unit)


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def state(position, error=0., rotation=(0., 0., 0., 1.), e_rotation=0.):
    return tolerance.Tracked(position, rotation, 1., error, e_rotation, 0.)


def emulated_link(parent, t, q, s):
    """Unity's link with every operation rounded to f32, in the order the operation model counts."""
    (x, y, z, w), r = parent[1], f32

    def cross(a, b):
        return (r(r(a[1] * b[2]) - r(a[2] * b[1])), r(r(a[2] * b[0]) - r(a[0] * b[2])),
                r(r(a[0] * b[1]) - r(a[1] * b[0])))
    u = (x, y, z)
    tt = tuple(2 * c for c in cross(u, t))
    ut = cross(u, tt)
    turned = tuple(r(r(a + r(w * b)) + c) for a, b, c in zip(t, tt, ut))
    offset = tuple(r(parent[2] * c) for c in turned)
    position = tuple(r(a + b) for a, b in zip(parent[0], offset))
    (ax, ay, az, aw), (bx, by, bz, bw) = parent[1], q

    def dot(*pairs):
        total = 0.
        for a, b in pairs:
            total = r(total + r(a * b))
        return total
    rotation = (dot((aw, bx), (ax, bw), (ay, bz), (-az, by)), dot((aw, by), (-ax, bz), (ay, bw), (az, bx)),
                dot((aw, bz), (ax, by), (-ay, bx), (az, bw)), dot((aw, bw), (-ax, bx), (-ay, by), (-az, bz)))
    return position, rotation, r(parent[2] * s)


class OperationTests(unittest.TestCase):
    def test_one_link_from_the_identity_by_hand(self):
        t, q = (.3, -.4, 1.2), (0., 0., 0., 1.)
        child = tolerance.link(tolerance.IDENTITY, t, q, 1.)
        # The identity rotation: kappa |dq| = 0, the rotation rounds at |t|, the scale product and the
        # add at |t|, and the product of rotations at |q| = 1.
        want = G("rotate") * 1.3 + U * 1.3 + U * 1.3 + 10 * 2. ** -126
        self.assertAlmostEqual(child.e_position, want, delta=1e-22)
        self.assertEqual(child.e_rotation, gamma(4))
        self.assertEqual(child.e_scale, U)

    def test_the_turn_sensitivity_and_rotation_rounding(self):
        self.assertEqual(tolerance.turn_sensitivity((0., 0., 0., 1.)), 2.)
        self.assertAlmostEqual(tolerance.turn_sensitivity((1., 0., 0., 0.)), 2 * math.sqrt(5), places=15)
        # Each bound covers a small perturbation of the quaternion.
        rng = random.Random(3)
        for _ in range(50):
            q = euler(tuple(rng.uniform(-180, 180) for _ in range(3)), 4)
            dq = tuple(rng.uniform(-1e-4, 1e-4) for _ in range(4))
            v = tuple(rng.uniform(-1, 1) for _ in range(3))
            moved = tolerance.sub(tolerance.unity_rotate(tuple(a + b for a, b in zip(q, dq)), v),
                                  tolerance.unity_rotate(q, v))
            bound = tolerance.turn_sensitivity(q) * tolerance.norm(dq) * tolerance.norm(v)
            self.assertLessEqual(tolerance.norm(moved), bound * (1 + 1e-3))

    def test_emulated_f32_chains_stay_within_the_tracked_bounds(self):
        rng = random.Random(7)
        for _ in range(200):
            exact, emulated = tolerance.IDENTITY, ((0., 0., 0.), (0., 0., 0., 1.), 1.)
            for _ in range(rng.randint(1, 8)):
                t = tuple(f32(rng.uniform(-.6, .6)) for _ in range(3))
                q = tuple(map(f32, euler(tuple(rng.uniform(-90, 90) for _ in range(3)), 4)))
                s = f32(rng.choice((1., 1., rng.uniform(.5, 1.5))))
                exact = tolerance.link(exact, t, q, s)
                emulated = emulated_link(emulated, t, q, s)
                self.assertLessEqual(tolerance.norm(tolerance.sub(emulated[0], exact.position)), exact.e_position)
                self.assertLessEqual(tolerance.norm(tuple(a - b for a, b in zip(emulated[1], exact.rotation))),
                                     exact.e_rotation)
                self.assertLessEqual(abs(emulated[2] - exact.scale), exact.e_scale)

    def test_hips_local_bounds_do_not_depend_on_where_the_hips_are(self):
        doc = body_doc("body-probes-compact", RUN, samples=1)
        nodes, transforms = doc["spec"]["nodes"], doc["capture"]["samples"][0]["pose"]["transforms"]
        human = doc["capture"]["humanoid"]["bones"]
        hips = next(node for node in nodes if node["name"] == "Hips")["address"]
        links = tolerance.recorded_edges(human, nodes)
        near = tolerance.body_states(nodes, transforms, F32, human, links, hips)
        moved = json.loads(json.dumps(transforms))
        moved[[node["address"] for node in nodes].index(hips)]["translation"] = [replay.f32_bits(v) for v in (90, 3, -50)]
        far = tolerance.body_states(nodes, moved, F32, human, links, hips)
        self.assertEqual({n: b.e_position for n, b in near[0].items()}, {n: b.e_position for n, b in far[0].items()})
        self.assertEqual(near[1], far[1])
        self.assertEqual(near[0]["Hips"].e_position, 0.)
        self.assertLess(near[0]["Spine"].e_position, near[0]["LeftHand"].e_position)
        # An edge difference's error excludes the parent's own position error.
        self.assertLess(near[1]["LeftLowerArm", "LeftHand"], near[0]["LeftHand"].e_position)


class BoundTests(unittest.TestCase):
    def test_centre_of_mass_bound_by_hand(self):
        # c = sum m_i (p_i + sum_j a_ij (p_j - p_i)): the bones' errors and each edge difference's.
        bones = {"Hips": state((0., 0., 0.)), "Spine": state((0., .2, 0.), 2e-6),
                 "LeftUpperLeg": state((-.1, -.05, 0.), 3e-6)}
        differences = {("Hips", "Spine"): 1e-6, ("Hips", "LeftUpperLeg"): 1.5e-6}
        masses = {"Hips": .5, "Spine": .3, "LeftUpperLeg": .2}
        links = [("Hips", "Spine"), ("Hips", "LeftUpperLeg")]
        error, magnitude = tolerance.com_error(bones, masses, links, differences)
        want_magnitude = .5 * (.2 + math.hypot(.1, .05)) + .3 * .2 + .2 * math.hypot(.1, .05)
        self.assertAlmostEqual(magnitude, want_magnitude, places=15)
        want = .5 * 2.5e-6 + .3 * 2e-6 + .2 * 3e-6 + gamma(2 + 6 + 2) * want_magnitude
        self.assertAlmostEqual(error, want, delta=1e-20)

    def test_frame_bound_by_hand(self):
        points = dict(zip(tolerance.FRAME_BONES, ((-1, 1, 0), (1, 1, 0), (-1, -1, 0), (1, -1, 0))))
        e = 1e-6
        errors = dict.fromkeys(tolerance.FRAME_BONES, e)
        operands = math.hypot(4, 4)  # componentwise |ul| + |ur| + |ll| + |lr|
        d_up = 2 * (4 * e / 2 + gamma(2) * operands / 2) / 2 + gamma(6)
        # lateral (4, 0, 0) x up (0, 1, 0): its operand cross is (0, 0, 4); forward (0, 0, 1) x up: (1, 0, 0).
        d_cross = 4 * e + gamma(2) * operands + 4 * d_up + gamma(2) * 4
        d_forward = 2 * d_cross / 4 + gamma(6)
        d_right = d_up + d_forward + gamma(2) * 1
        self.assertAlmostEqual(tolerance.frame_error(points, errors), math.sqrt(d_right ** 2 + d_up ** 2 + d_forward ** 2),
                               delta=1e-20)

    def test_frame_bound_grows_as_the_cross_product_vanishes(self):
        errors = dict.fromkeys(tolerance.FRAME_BONES, 1e-6)
        bounds = []
        for lean in (1., .1, .01):
            # Each side leans toward the up vector, so lateral x up shrinks with the lean.
            points = dict(zip(tolerance.FRAME_BONES, ((-lean, 1 - 1 / lean, 0.), (lean, 1 + 1 / lean, 0.),
                                                      (-lean, -1 - 1 / lean, 0.), (lean, -1 + 1 / lean, 0.))))
            bounds.append(tolerance.frame_error(points, errors))
        self.assertLess(bounds[0], bounds[1])
        self.assertLess(bounds[1], bounds[2])
        with self.assertRaisesRegex(ValueError, "lateral along up"):
            tolerance.frame_error(dict(zip(tolerance.FRAME_BONES, ((0, 2, 0), (0, 3, 0), (0, -2, 0), (0, -1, 0)))),
                                  errors)

    def test_readback_charges_the_world_magnitude_only_where_it_is_consumed(self):
        # An exact Animator at (2, 0, 0) with scale 2: the store and h w at |w|, the subtraction and the
        # inverse rotation at |d|, the division and the stored h division at |d| / s.
        root = tolerance.Tracked((2., 0., 0.), (0., 0., 0., 1.), 2.)
        world, h = (2.5, 1., 0.), 1.
        d = math.hypot(.5, 1.)
        want = (2 * U * math.hypot(2.5, 1.) + U * d + G("rotate") * d) / 2 + 2 * U * d / 2
        self.assertAlmostEqual(tolerance.readback_error(h, world, root), want, delta=1e-22)
        bones = {"Hips": state((2.5, 1., 0.), 1e-6)}
        self.assertAlmostEqual(tolerance.measurement_tolerance(bones, {}, {"Hips": 1.}, [], h, world, root),
                               (1e-6 + gamma(0 + 2 + 2) * math.hypot(2.5, 1.)) / 2 + want, delta=1e-22)

    def test_the_hips_parent_chain_is_budgeted(self):
        # The same parent exact and with errors: t_p's error, R_p's acting on P_A - t_p, and sigma_p's on
        # h T_A - t_p only, since it cancels in k / sigma_p.
        doc = body_doc("body-probes-compact", RUN, samples=1)
        nodes, transforms = doc["spec"]["nodes"], doc["capture"]["samples"][0]["pose"]["transforms"]
        human = doc["capture"]["humanoid"]["bones"]
        hips = next(node for node in nodes if node["name"] == "Hips")["address"]
        links = tolerance.recorded_edges(human, nodes)
        bones, differences = tolerance.body_states(nodes, transforms, F32, human, links, hips)
        masses = tolerance.folded_masses({b: support.MASSES[b] for b in tolerance.BODY_SLOTS}, support.PARENT,
                                         set(bones))
        frame = tolerance.body_frame({n: b.position for n, b in bones.items()})
        target, local = (0., 1., 0.), (0., .88, 0.)
        exact = tolerance.Tracked((.1, .2, .3), (0., 0., 0., 1.), 1.)
        noisy = tolerance.Tracked((.1, .2, .3), (0., 0., 0., 1.), 1., 1e-6, 2e-6, 3e-6)
        args = (bones, differences, bones, masses, links, 1., target, 0., 0., (0., 0., 0., 1.), frame)
        gap = (tolerance.hips_error(*args, noisy, 1., local)[0] - tolerance.hips_error(*args, exact, 1., local)[0])
        want = 1e-6 + 2. * 2e-6 * .88 + 3e-6 * math.hypot(.1, .8, .3)
        self.assertAlmostEqual(gap, want, delta=want * 1e-9)

    def test_the_model_evaluation_is_the_f64_path_and_one_rounding_of_its_own_translation(self):
        self.assertEqual(G("rotate", 1, tolerance.U64), gamma(6, 2. ** -53))
        doc = replay.read_json(json.dumps(body_doc("body-probes-compact", RUN, samples=1)))
        parents, masses = adequacy.bone_table([doc])
        sample = next(adequacy.samples("body-probes-compact", doc, parents, masses))
        hips = doc["spec"]["nodes"].index(next(node for node in doc["spec"]["nodes"] if node["name"] == "Hips"))
        local = tolerance.decode(doc["capture"]["samples"][0]["pose"]["transforms"][hips]["translation"], 3, F32)
        # The store's allowance bounds the model's pre-cast translation, at least the recorded one here.
        self.assertGreater(sample["evaluation"], U * tolerance.norm(local))
        self.assertLess(sample["evaluation"], 4 * U * tolerance.norm(local) + sample["unity"] * 2. ** -25)

    def test_edges_and_folded_masses(self):
        parents = support.PARENT
        links = tolerance.edges(parents, body.rig_bones("compact"))
        self.assertIn(("Hips", "Spine"), links)
        self.assertIn(("UpperChest", "Neck"), links)
        self.assertNotIn("LeftHand", {i for i, _ in links})
        mapped = body.rig_bones("no-chest")
        self.assertEqual({j for i, j in tolerance.edges(parents, mapped) if i == "Spine"},
                         {"Neck", "LeftShoulder", "RightShoulder"})
        masses = {name: support.MASSES[name] for name in tolerance.BODY_SLOTS}
        folded = tolerance.folded_masses(masses, parents, mapped)
        self.assertEqual(list(folded), tolerance.ordered(mapped))
        self.assertAlmostEqual(sum(folded.values()), 1, places=15)
        total = sum(masses.values())
        eyes_jaw = sum(masses[name] for name in ("LeftEye", "RightEye", "Jaw"))
        self.assertAlmostEqual(folded["Spine"], (masses["Spine"] + masses["Chest"] + masses["UpperChest"]) / total)
        self.assertAlmostEqual(folded["Head"], (masses["Head"] + eyes_jaw) / total)

    def test_recorded_edges_follow_the_hierarchy(self):
        doc = body_doc("body-probes-no-chest", RUN, samples=1)
        human, nodes = doc["capture"]["humanoid"]["bones"], doc["spec"]["nodes"]
        mapped = body.rig_bones("no-chest")
        self.assertEqual(tolerance.recorded_edges(human, nodes), tolerance.edges(support.PARENT, mapped))
        with self.assertRaisesRegex(ValueError, "nonuniform"):
            tolerance.uniform((1., 1., 1.5))


if __name__ == "__main__":
    unittest.main()
