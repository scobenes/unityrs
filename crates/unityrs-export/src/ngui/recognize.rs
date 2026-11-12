//! Section 5's recognition 2: whether an atlas's typetree has an `mSprites`
//! that UnityPy 1.25.2's `read_value` reads as a list of sprites, decided
//! from the nodes alone, before any value is read.

use unityrs_core::typetree::tree::py_kind_of;
use unityrs_core::typetree::{Kind, NodeId, NodeList};

/// Whether `FUNCTION_READ_MAP` (`helpers/TypeTreeHelper.py`) reads
/// `ty`: the core's table of the kinds the pure-Python reader dispatches on
/// ([`py_kind_of`]), its primitives, `string` and `TypelessData`.
fn in_read_map(ty: &[u8]) -> bool {
    is_integer(ty)
        || matches!(
            py_kind_of(ty),
            Kind::F32 | Kind::F64 | Kind::Bool | Kind::Str | Kind::Bytes
        )
}

/// Whether `ty` is an integer type of `FUNCTION_READ_MAP`.
#[must_use]
pub fn is_integer(ty: &[u8]) -> bool {
    matches!(
        py_kind_of(ty),
        Kind::S8 | Kind::U8 | Kind::S16 | Kind::U16 | Kind::S32 | Kind::U32 | Kind::S64 | Kind::U64
    )
}

/// Whether `read_value` reads `ty` by its own branch before the vector and
/// class branches: `FUNCTION_READ_MAP`, `pair`, `ReferencedObject`.
fn read_before_vector(ty: &[u8]) -> bool {
    in_read_map(ty) || matches!(py_kind_of(ty), Kind::Pair | Kind::ReferencedObject)
}

/// The last child of `id` named `name`, raw: the one whose value a
/// typetree's dict keeps under that name (`parse_as_dict`; review round 2's
/// C2: an atlas is read as a dict, not an object).
fn last_child(list: &NodeList, id: NodeId, name: &[u8]) -> Option<NodeId> {
    list.children(id)
        .iter()
        .rev()
        .find(|c| list.name(**c) == name)
        .copied()
}

/// Whether `read_value` reads `id` as a vector: none of the earlier
/// branches, and a first child of type `Array`.
fn is_vector(list: &NodeList, id: NodeId) -> bool {
    !read_before_vector(list.type_name(id))
        && list
            .children(id)
            .first()
            .is_some_and(|a| list.type_name(*a) == b"Array")
}

/// Recognition 2 over a typetree's nodes.
#[must_use]
pub fn has_sprite_list(list: &NodeList) -> bool {
    let Some(sprites) = last_child(list, list.root(), b"mSprites") else {
        return false;
    };
    if !is_vector(list, sprites) {
        return false;
    }
    let Some(array) = list.children(sprites).first() else {
        return false;
    };
    let Some(&element) = list.children(*array).get(1) else {
        return false;
    };
    if read_before_vector(list.type_name(element)) || is_vector(list, element) {
        return false;
    }
    let int = |name: &[u8]| {
        last_child(list, element, name).is_some_and(|c| is_integer(list.type_name(c)))
    };
    int(b"x")
        && int(b"y")
        && int(b"width")
        && int(b"height")
        && last_child(list, element, b"name").is_some_and(|c| list.type_name(c) == b"string")
}

#[cfg(test)]
mod tests {
    use unityrs_core::typetree::{NodeList, NodeRow};

    use super::{has_sprite_list, is_integer};

    fn list(rows: &[(i32, &str, &str)]) -> NodeList {
        NodeList::trusted(
            rows.iter()
                .map(|(l, t, n)| NodeRow::new(*l, *t, *n))
                .collect(),
        )
        .unwrap()
    }

    fn atlas(element: &str, x: &str, extra: &[(i32, &str, &str)]) -> NodeList {
        let mut rows = vec![
            (0, "MonoBehaviour", "Base"),
            (1, "vector", "mSprites"),
            (2, "Array", "Array"),
            (3, "int", "size"),
            (3, element, "data"),
            (4, "string", "name"),
            (5, "Array", "Array"),
            (6, "int", "size"),
            (6, "char", "data"),
            (4, x, "x"),
            (4, "int", "y"),
            (4, "SInt64", "width"),
            (4, "UInt16", "height"),
        ];
        rows.extend_from_slice(extra);
        list(&rows)
    }

    #[test]
    fn a_struct_of_integer_rectangles_and_a_string_name_is_an_atlas() {
        assert!(has_sprite_list(&atlas("UISpriteData", "int", &[])));
        assert!(has_sprite_list(&atlas("PPtr<X>", "FileSize", &[])));
    }

    #[test]
    fn other_shapes_are_not() {
        assert!(!has_sprite_list(&atlas("UISpriteData", "float", &[])));
        assert!(!has_sprite_list(&atlas("UISpriteData", "bool", &[])));
        assert!(!has_sprite_list(&atlas("pair", "int", &[])));
        assert!(!has_sprite_list(&atlas("string", "int", &[])));
        // The last `x` decides: a later float `x` hides the int one.
        assert!(!has_sprite_list(&atlas(
            "UISpriteData",
            "int",
            &[(4, "float", "x")]
        )));
        // The last `mSprites` decides.
        assert!(!has_sprite_list(&atlas(
            "UISpriteData",
            "int",
            &[(1, "int", "mSprites")]
        )));
        // By raw name: an `x?` beside `x`, or an `mSprites?` beside
        // `mSprites`, is another field.
        assert!(has_sprite_list(&atlas(
            "UISpriteData",
            "int",
            &[(4, "float", "x?"), (1, "int", "mSprites?")]
        )));
        // A `string` named mSprites reads as a string, not a vector.
        let s = list(&[
            (0, "MonoBehaviour", "Base"),
            (1, "string", "mSprites"),
            (2, "Array", "Array"),
            (3, "int", "size"),
            (3, "char", "data"),
        ]);
        assert!(!has_sprite_list(&s));
        assert!(!has_sprite_list(&list(&[(0, "MonoBehaviour", "Base")])));
    }

    #[test]
    fn integers_are_the_read_maps_less_five() {
        for t in ["SInt8", "char", "Type*", "unsigned long long", "FileSize"] {
            assert!(is_integer(t.as_bytes()), "{t}");
        }
        for t in ["float", "double", "bool", "string", "TypelessData", "Int"] {
            assert!(!is_integer(t.as_bytes()), "{t}");
        }
    }
}
