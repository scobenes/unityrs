"""Charge generated text and an AST upper bound before allocating either."""
import io
import tokenize

MAX_ANNOTATION_BYTES = 64 * 1024
MAX_ANNOTATION_TOTAL = 16 * 1024 * 1024
MAX_SOURCE_BYTES = 16 * 1024 * 1024
MAX_AST_NODES = 4_000_000
MAX_FIELDS = 250_000
MAX_CLASSES = 50_000
MAX_INHERITANCE_DEPTH = 64
MAX_MRO_TOTAL = 250_000
MAX_INHERITED_FIELDS = 250_000
MAX_FIELDS_PER_CLASS = 4096
MAX_WORK = 4_000_000
MAX_CACHE_RECORDS = 1_000_000
MAX_NAME_BYTES = 64 * 1024
MAX_STRING_WORK = 64 * 1024 * 1024


class ExpansionError(ImportError):
    """The supplied TPK requires excessive class-generation expansion."""


def refuse(kind):
    raise ExpansionError('class-generation ' + kind + ' expansion exceeds limit')


class Budget:
    def __init__(self):
        self.annotations = 0
        self.source = 0
        self.fields = 0
        self.classes = 0
        self.depth = 0
        self.work = 0
        self.cache_records = 0
        self.mro_total = 0
        self.inherited_fields = 0
        self.string_work = 0

    def string_charge(self, size):
        size *= 4
        if self.string_work + size > MAX_STRING_WORK:
            refuse('string work')
        self.string_work += size

    def name(self, value, kind):
        # Charge each use, including cached edges: distinct equal strings can
        # make hashing, dict/set lookups and field intersections scan all bytes.
        if len(value) * 4 > MAX_NAME_BYTES:
            refuse(kind + ' name')
        self.string_charge(len(value))
        return value

    def visit(self, count=1):
        if self.work + count > MAX_WORK:
            refuse('work')
        self.work += count

    def cache(self, count=1):
        if self.cache_records + count > MAX_CACHE_RECORDS:
            refuse('cache')
        self.cache_records += count

    def inheritance(self, bases, fields):
        # Summing bases deliberately overcounts shared ancestors. This bounds
        # MRO construction and attrs/ABC scans without materializing the MRO.
        depth = 1 + max((base[0] for base in bases), default=1)
        mro = 1 + sum(base[1] for base in bases) if bases else 2
        fields += sum(base[2] for base in bases)
        if depth > MAX_INHERITANCE_DEPTH:
            refuse('inheritance depth')
        if self.mro_total + mro > MAX_MRO_TOTAL:
            refuse('inheritance MRO')
        if fields > MAX_FIELDS_PER_CLASS or self.inherited_fields + fields > MAX_INHERITED_FIELDS:
            refuse('inherited field')
        self.mro_total += mro
        self.inherited_fields += fields
        return depth, mro, fields

    def annotation(self, *parts):
        self.annotation_size(sum(len(part) for part in parts))
        return ''.join(parts)

    def annotation_size(self, size):
        self.string_charge(size)
        size *= 4
        if size > MAX_ANNOTATION_BYTES or self.annotations + size > MAX_ANNOTATION_TOTAL:
            refuse('annotation')
        self.annotations += size

    def source_charge(self, size):
        # Four bytes per code point bounds UTF-8 without encoding a copy.
        size *= 4
        if self.source + size > MAX_SOURCE_BYTES:
            refuse('source')
        self.source += size

    def field(self):
        if self.fields >= MAX_FIELDS:
            refuse('field count')
        self.fields += 1

    def class_record(self):
        if self.classes >= MAX_CLASSES:
            refuse('class count')
        self.classes += 1


def check_source(source):
    if len(source) * 4 > MAX_SOURCE_BYTES:
        refuse('source')
    # Each token contributes at most four AST nodes in the accepted grammar
    # (names, contexts, subscripts, tuples and class/field declarations).
    # Reserve this conservative count before ast.parse builds any nodes.
    count = 0
    for token in tokenize.generate_tokens(io.StringIO(source).readline):
        # An f-string can hide expressions inside a single STRING token.
        count += 4 * (len(token.string) if token.type == tokenize.STRING else 1)
        if count > MAX_AST_NODES:
            refuse('AST')
