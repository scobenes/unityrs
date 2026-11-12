/* Ported from zlib 1.2.13 (Zlib): deflate.c's non-FASTEST longest_match.
 * Copyright (C) 1995-2022 Jean-loup Gailly and Mark Adler
 * Altered source version: unityrs replaces the comparison loop with
 * memcpy word loads. This is not the original zlib source.
 * For conditions of distribution and use, see ../LICENSE-ZLIB.
 */

#include <stdint.h>
#include <string.h>

local uint16_t unityrs_match_load16(const Bytef *p)
{
    uint16_t value;
    memcpy(&value, p, sizeof(value));
    return value;
}

local uInt longest_match(s, cur_match)
    deflate_state *s;
    IPos cur_match;
{
    unsigned chain_length = s->max_chain_length;
    Bytef *scan = s->window + s->strstart;
    Bytef *match;
    int len;
    int best_len = (int)s->prev_length;
    int nice_match = s->nice_match;
    IPos limit = s->strstart > (IPos)MAX_DIST(s) ?
        s->strstart - (IPos)MAX_DIST(s) : NIL;
    Posf *prev = s->prev;
    uInt wmask = s->w_mask;
    uint16_t scan_start = unityrs_match_load16(scan);
    uint16_t scan_end = unityrs_match_load16(scan + best_len - 1);

    Assert(s->hash_bits >= 8 && MAX_MATCH == 258, "Code too clever");
    Assert((ulg)s->strstart <= s->window_size - MIN_LOOKAHEAD,
           "need lookahead");

    if (s->prev_length >= s->good_match) chain_length >>= 2;
    if ((uInt)nice_match > s->lookahead) nice_match = (int)s->lookahead;

    do {
        Assert(cur_match < s->strstart, "no future");
        match = s->window + cur_match;

        /* These are exactly the original non-UNALIGNED path's four byte
         * checks, grouped into two endian-independent equality tests.
         * memcpy permits unaligned addresses and does not alias a Bytef
         * window through a different type.
         */
        if (unityrs_match_load16(match + best_len - 1) != scan_end ||
            unityrs_match_load16(match) != scan_start) continue;

        /* The hash and the first two bytes imply scan[2] == match[2],
         * just as in zlib. The length is the first mismatch from offset 3,
         * capped at MAX_MATCH. Keep candidate order, strict improvement,
         * chain budget, nice_match and the lookahead clamp unchanged:
         * this preserves both the return and match_start (including ties).
         * All reads end at or before scan + 258, which is inside the
         * window: strstart <= window_size - MIN_LOOKAHEAD (262). Match
         * precedes scan, so its reads are within the window too.
         */
        Assert(scan[2] == match[2], "match[2]?");
        len = 3;
#if !defined(UNITYRS_MATCH_BYTEWISE) && \
    defined(__BYTE_ORDER__) && defined(__ORDER_LITTLE_ENDIAN__) && \
    __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__ && \
    (defined(__GNUC__) || defined(__clang__))
        /* Only little-endian makes trailing zero bits count equal leading
         * bytes. Unknown/big endian and compilers without ctzll use the
         * byte loop below. UNITYRS_MATCH_BYTEWISE exercises that fallback.
         */
        /* Offsets are 3, 11, ..., 251. The last load ends at offset 258,
         * which the original byte loop also reads. MIN_LOOKAHEAD leaves
         * room for it; discard that extra byte by capping the result.
         */
        while (len < MAX_MATCH) {
            uint64_t a, b, difference;
            memcpy(&a, scan + len, sizeof(a));
            memcpy(&b, match + len, sizeof(b));
            difference = a ^ b;
            if (difference != 0) {
                len += (int)(__builtin_ctzll(difference) >> 3);
                break;
            }
            len += 8;
        }
        if (len > MAX_MATCH) len = MAX_MATCH;
#else
        while (len < MAX_MATCH && scan[len] == match[len]) len++;
#endif

        if (len > best_len) {
            s->match_start = cur_match;
            best_len = len;
            if (len >= nice_match) break;
            scan_end = unityrs_match_load16(scan + best_len - 1);
        }
    } while ((cur_match = prev[cur_match & wmask]) > limit
             && --chain_length != 0);

    if ((uInt)best_len <= s->lookahead) return (uInt)best_len;
    return s->lookahead;
}
