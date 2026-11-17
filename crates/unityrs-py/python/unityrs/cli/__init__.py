# Ported from UnityPy 1.25.2 (MIT): cli/__init__.py:1-28. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Explicit compatibility utilities; importing this module performs no I/O."""
from argparse import ArgumentParser
from .update_tpk import update_tpk


def main():
    parser = ArgumentParser(prog='unityrs', description='unityrs cli utility')
    subparsers = parser.add_subparsers(title='utils')
    update = subparsers.add_parser('update_tpk', help='Updates TPK (typetree dump) file')
    update.set_defaults(func=update_tpk)
    args = parser.parse_args()
    if not hasattr(args, 'func'):
        parser.print_help()
        return
    args.func(*getattr(args, 'operands', []))


if __name__ == '__main__':
    main()
