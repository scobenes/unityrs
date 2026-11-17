#!/usr/bin/env python3
"""Run native synthetic calibration serially; retain and publish aggregates only.

Builds finish before any measured scene. Child output stays in memory, is
parsed through a closed numeric protocol and is never attached or echoed.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/parity'))
import child
import regenerate as gen

# Fixed envelopes have separate source/structural and native component gates;
# they remain ceilings rather than values fitted to a process residual.
BATCH_TARGETS = ('memory_constants', 'memory_gate', 'store_memory_gate',
                 'memory_retention', 'limit_squeeze',
                 'http_alloc', 'store_alloc', 'store_ops_alloc', 'store_list_alloc')
NATIVE_TARGETS = ('memory_model', 'heap_rounding', 'lzma_decoder_heap')


def check_build_environment(environment):
    exact = {'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC', 'RUSTDOC',
             'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTC_BOOTSTRAP',
             'CC', 'CXX', 'AR', 'CFLAGS', 'CXXFLAGS', 'CPPFLAGS', 'LDFLAGS',
             'LD_PRELOAD', 'LD_LIBRARY_PATH', 'GLIBC_TUNABLES',
             'CRATE_CC_NO_DEFAULTS', 'GCC_EXEC_PREFIX', 'COMPILER_PATH',
             'LIBRARY_PATH', 'CPATH', 'C_INCLUDE_PATH', 'CPLUS_INCLUDE_PATH'}
    prefixes = ('CC_', 'CXX_', 'AR_', 'CFLAGS_', 'CXXFLAGS_', 'CARGO_PROFILE_', 'CARGO_BUILD_',
                'MALLOC_', 'UNITYRS_TEST_', 'B12_', 'X12_', 'HOST_', 'TARGET_')
    if any(value and (key in exact or key.startswith(prefixes)
                      or (key.startswith('CARGO_TARGET_') and key != 'CARGO_TARGET_DIR'))
           for key, value in environment.items()):
        raise gen.Invalid('uncontrolled build or allocator override')

# A successful command is necessary but insufficient: every named scene must
# actually have been discovered and run. Missing coverage is never inferred.
COVERAGE = {
    'worker-count': ('initialized_workers_fit_the_worker_increment',),
    'pool-controller': ('pools::actual_lookup_cells_on_both_pools_fit_the_pool_increment',),
    'pool-finisher': ('pools::actual_lookup_cells_on_both_pools_fit_the_pool_increment',),
    'environment': ('a_private_environments_fixed_parts_fit_c_env_whatever_d',),
    'full-dispatch-channel': ('full_dispatch_channels_include_fixed_overhead_at_small_capacities',),
    'full-end-channel': ('a_full_end_channel_fits_threads_times_m_end_at_every_capacity',),
    'base-startup': ('startup_without_test_hooks_has_no_rust_heap',
                     'directory_base_measurement_keeps_all_conservative_overlap_visible',
                     'store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup'),
    'base-failed-input': ('store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup',),
    'base-archive': ('store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup',),
    'base-old-journal': ('store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup',),
    'base-resume': ('store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup',),
    'base-lookup': ('store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup',),
    'normal-run': ('directory_and_http_processes_count_all_threads_and_live_charges',
                   'store_process_counts_signed_pages_verification_and_final_head'),
    'loose-publication': ('process_export_matrix_directory_http_and_pool_counts',),
    'zip-publication': ('output_and_zip_member_growth_leaves_no_count_proportional_residual',),
    'http-tls': ('tls12_near_cap_http_and_reframed_handshake_are_counted_in_process',),
    'view-paths': ('full_size_view_vectors_fit_their_structural_share',
                   'env::registry::dependency::batch_view_inventory::uncharged_dependency_owners_fit_the_bounded_view_share'),
    'header-ownership': ('stage::env::tests::variable_table_growth_fits_headers_at_their_live_value_sizes',
                         'stage::env::tests::repeated_headers_fit_measured_masker_storage_without_later_growth'),
    'scale-residual': ('output_and_zip_member_growth_leaves_no_count_proportional_residual',
                       'sequential_hostnames_and_retries_leave_no_completed_request_history',
                       'store_output_and_page_counts_leave_no_retained_history'),
    'native-components': ('liblzmas_decoder_is_charged',),
    'retention': ('rss_tracks_live_heap_after_large_frees_with_and_without_settings',),
    'store-multipart': ('store_multipart_normal_and_stopped_transfers_remain_counted',),
    'store-list': ('store_process_counts_signed_pages_verification_and_final_head',),
    'store-verification': ('store_process_counts_signed_pages_verification_and_final_head',
                           'store_error_bodies_and_retry_verification_failure_remain_bounded'),
    'store-journal': ('store_resume_and_journal_rewrite_return_all_ownership',
                     'store_stop_during_old_journal_abort_retains_its_counted_identity'),
    'store-final-head': ('store_process_read_back_and_rewrite_stays_bounded',
                         'finisher_rows_larger_than_one_chunk_stay_inside_twenty_four_mib',
                         'store_stop_inside_finisher_rows_and_final_head_keeps_its_normal_snapshot',
                         'six_capped_credential_holders_share_one_bounded_provider'),
    **{'stop-phase-' + str(n): ('every_phase_stop_keeps_the_counted_owners_through_exit',
                                'store_stops_in_every_phase_retain_counted_owners')
       for n in range(1, 8)},
}


def coverage(text, complete=True):
    discovered = set()
    for match in re.finditer(r'^test ([a-zA-Z0-9_:]+) \.\.\.(.*?)(?=^test |\Z)',
                             text, re.MULTILINE | re.DOTALL):
        body = match[2].strip()
        if body.endswith('ok') and not body.startswith('ignored'):
            discovered.add(match[1])
    result = {scene: all(name in discovered for name in names)
              for scene, names in COVERAGE.items()}
    if set(result) != set(gen.SCENES):
        raise gen.Invalid('coverage catalogue and schema differ')
    if complete and not all(result.values()):
        raise gen.Invalid('incomplete synthetic scene coverage')
    return result


def run(command, *, env=None):
    result = child.run(command, cwd=ROOT, env=env, stdout=child.PIPE, stderr=child.PIPE)
    if result.returncode:
        # The command's own words (fixed in this file) and its status name
        # the failing step; the child's output is never echoed.
        raise gen.Invalid(f'calibration command failed ({label(command)}, status {result.returncode})')
    return result.stdout.decode('utf-8', errors='strict')


def label(command):
    """The command as this file spells it, without paths, at most 240 characters."""
    words = ' '.join(word for word in command if '/' not in word)
    return words if len(words) <= 240 else words[:237] + '...'


def numeric_protocol(text):
    """Accept only known, unsigned numeric observations, taking maxima."""
    values = {'COEFFICIENT': {}, 'FLOOR': {}, 'ENVELOPE': {}}
    keys = {'COEFFICIENT': gen.COEFFICIENTS,
            'FLOOR': ('m_dispatch', 'm_end'), 'ENVELOPE': gen.ENVELOPES}
    for line in text.splitlines():
        if not line.startswith('B12_'):
            continue
        match = re.fullmatch(r'B12_(COEFFICIENT|FLOOR|ENVELOPE) ([a-z_]+) ([0-9]+)', line)
        if match is None:
            raise gen.Invalid('invalid numeric calibration protocol')
        kind, name, raw = match.groups()
        number = int(raw)
        if name not in keys[kind] or number > gen.U64_MAX:
            raise gen.Invalid('invalid numeric calibration observation')
        values[kind][name] = max(number, values[kind].get(name, 0))
    return values


def native_fingerprint():
    """Canonical native source/configuration identity, identical across targets."""
    paths = ['Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml']
    for name in ('.cargo/config', '.cargo/config.toml'):
        if (ROOT / name).is_file():
            paths.append(name)
    for directory in ('unityrs-compress-sys', 'unityrs-imageio', 'unityrs-texture'):
        root = ROOT / 'crates' / directory
        paths.extend(str(p.relative_to(ROOT)) for p in root.rglob('*') if p.is_file()
                     and p.suffix.lower() in ('.rs', '.toml', '.c', '.h', '.cc', '.cpp', '.hpp', '.s'))
    digest = hashlib.sha256()
    for name in sorted(set(paths)):
        digest.update(name.encode('utf-8') + b'\0')
        digest.update(hashlib.sha256((ROOT / name).read_bytes()).digest())
    return digest.hexdigest()


def version(text, count):
    match = re.search(r'([0-9]+)\.([0-9]+)(?:\.([0-9]+))?', text)
    if match is None:
        raise gen.Invalid('unsupported host version')
    return [int(n or 0) for n in match.groups()][:count]


def provenance(native, allow_dirty):
    check_build_environment(os.environ)
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    if any((cargo_home / name).is_file() and (cargo_home / name).stat().st_size
           for name in ('config', 'config.toml')):
        raise gen.Invalid('external Cargo configuration is ineligible')
    if any((directory / '.cargo' / name).is_file()
           for directory in ROOT.parents for name in ('config', 'config.toml')):
        raise gen.Invalid('ancestor Cargo configuration is ineligible')
    if platform.system() != 'Linux' or platform.machine() not in ('x86_64', 'aarch64'):
        raise gen.Invalid('calibration requires a supported Linux host')
    rustc = run(['rustc', '-vV'])
    pinned = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    if version(rustc, 3) != version(pinned, 3):
        raise gen.Invalid('calibration requires the pinned Rust toolchain')
    target = platform.machine() + '-unknown-linux-gnu'
    if 'host: ' + target not in rustc:
        raise gen.Invalid('cross-target calibration is ineligible')
    status = run(['git', 'status', '--porcelain', '--untracked-files=normal'])
    clean = not status.strip()
    if not clean and not allow_dirty:
        raise gen.Invalid('calibration requires clean source')
    page = os.sysconf('SC_PAGESIZE')
    if page != 4096:
        raise gen.Invalid('calibration requires 4096-byte pages')
    compilers = {}
    for name, command in (('c', 'cc'), ('cxx', 'c++')):
        triple = run([command, '-dumpmachine']).strip()
        if not triple.startswith(platform.machine() + '-') or not triple.endswith('linux-gnu'):
            raise gen.Invalid('unsupported native compiler target')
        compilers[name] = version(run([command, '-dumpfullversion']), 3)
    return target, {
        'revision': run(['git', 'rev-parse', 'HEAD']).strip(),
        'lockfile_sha256': hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest(),
        'rustc': version(rustc, 3), 'glibc': version(platform.libc_ver()[1], 2),
        'kernel': version(platform.release(), 3),
        'native_libraries_sha256': native_fingerprint(), 'features': 'test-hooks',
        'native_compilers': compilers,
        'native_flags_sha256': hashlib.sha256(b'default-locked-profiles-no-overrides-v1').hexdigest(),
        'allocator': 'system-glibc', 'native': native, 'clean': clean,
        'page_size': page, 'mmap_threshold': 131072, 'arena_extra': 2,
    }


def commands(profile):
    profile_args = ['--release'] if profile == 'release' else []
    batch = ['cargo', 'test', '--locked', *profile_args, '-p', 'unityrs-batch', '--features', 'test-hooks']
    for name in BATCH_TARGETS:
        batch.extend(['--test', name])
    core = ['cargo', 'test', '--locked', *profile_args, '-p', 'unityrs-core']
    for name in NATIVE_TARGETS:
        core.extend(['--test', name])
    return batch, core


def support_commands(profile):
    # Header ownership and the private view owners are the library evidence
    # needed here. Full unit regressions belong to their separate CI suites.
    library = ['cargo', 'test', '--locked', *(['--release'] if profile == 'release' else []),
               '-p', 'unityrs-batch', '--features', 'test-hooks', '--lib', 'stage::env::tests::']
    view = ['cargo', 'test', '--locked', *(['--release'] if profile == 'release' else []),
            '-p', 'unityrs-core', '--lib', 'uncharged_dependency_owners_fit_the_bounded_view_share']
    return library, view


def collect(profile, native, allow_dirty=False, smoke=False):
    target, source = provenance(native, allow_dirty)
    batch, core = commands(profile)
    library, view = support_commands(profile)
    # One build phase; no heavy compiler is left beside a measurement.
    for command in (batch, core, library, view):
        run([*command, '--no-run'])
    # Same PID namespace guard; local callers also check both containers.
    busy = child.run(['pgrep', 'controller_allo'], stdout=child.PIPE, stderr=child.PIPE)
    if busy.returncode != 1:
        raise gen.Invalid('allocation sweep active or process census unavailable')
    flags = ['--nocapture', '--test-threads=1', *([] if smoke else ['--include-ignored'])]
    text = run([*batch, '--', *flags])
    observations = numeric_protocol(text)
    text += run([*core, '--', '--include-ignored', '--test-threads=1'])
    text += run([*library, '--', '--include-ignored', '--test-threads=1'])
    text += run([*view, '--', '--test-threads=1'])
    covered = coverage(text, complete=not smoke)
    if set(observations['COEFFICIENT']) != set(gen.COEFFICIENTS):
        raise gen.Invalid('missing coefficient measurements')
    if set(observations['FLOOR']) != {'m_dispatch', 'm_end'}:
        raise gen.Invalid('missing structural measurements')
    records = []
    for sink in gen.SINKS:
        records.append({
            'kind': 'synthetic', 'target': target, 'profile': profile, 'sink': sink,
            'provenance': source, 'coverage': covered,
            'coefficients': observations['COEFFICIENT'],
            'structural_floors': observations['FLOOR'],
            'fixed_envelopes': {**gen.ENVELOPES, **observations['ENVELOPE']},
            'attribution': {
                'base_workers': 1, 'base_download_concurrency': 1,
                'held_charges_only': True, 'live_deductions_only': True,
                'raw_incremental_coefficients': True, 'native_only_charges_excluded': True,
                'observer_allocation_free': True, 'coherent_observations': True,
                'peer_excluded': True, 'parent_startup_included': True,
            },
        })
    data = {'schema_version': 1, 'status': 'provisional-b12b', 'records': records}
    schema = gen._schema('aggregate-input-v1.schema.json')
    gen._validate(data, schema, schema)
    # Diagnostic records remain schema-shaped but cannot enter selection.
    if native and source['clean']:
        gen.validate_input(data)
    return data


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=gen.PROFILES, required=True)
    label = parser.add_mutually_exclusive_group(required=True)
    label.add_argument('--native', action='store_true', help='assert native runner execution')
    label.add_argument('--emulated', action='store_true', help='diagnostic only; never eligible')
    parser.add_argument('--allow-dirty', action='store_true', help='diagnostic only; never eligible')
    parser.add_argument('--smoke', action='store_true', help='diagnostic only; leave scale coverage pending')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        data = collect(args.profile, args.native, args.allow_dirty, args.smoke)
        args.output.write_text(json.dumps(data, sort_keys=True, indent=2) + '\n', encoding='utf-8')
        print('synthetic aggregate written; no raw output retained')
        return 0
    except gen.Invalid as error:
        # Every Invalid message is a fixed string of the collector's or the
        # generator's, or a command label (run), never a child's output.
        print('calibration failed; no aggregate published', file=sys.stderr)
        print(f'reason: {error}', file=sys.stderr)
        return 2
    except (child.SystemKilled, OSError, UnicodeError, ValueError) as error:
        print('calibration failed; no aggregate published', file=sys.stderr)
        print(f'reason: {type(error).__name__}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
