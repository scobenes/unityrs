//! Exact third-party allocations reached by the store CR10 scopes.
//! These APIs expose no fallible storage constructor. `C_net` and phase envelopes
//! cover their bounded storage; allocator refusal is the documented A13 abort.
//! Both the innermost engine caller and its immediate dependency entry point
//! must match, so an engine allocation cannot borrow a dependency exemption.
use super::{Exclusion, Platform};

pub const INVENTORY: &[Exclusion] = &[
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::list::Listing::page"],
        platform: Platform::All,
        frame: Some(
            "<<alloc::vec::Vec<_> as serde_core::de::Deserialize>::deserialize::VecVisitor<object_store::client::s3::ListContents> as serde_core::de::Visitor>::visit_seq",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
        ],
        name: "store dependency <<alloc::vec::Vec<_> as serde_core::de::Deserialize>::deserialize::VecVisitor<object_store::client::s3::ListContents> as serde_core::de::Visitor>::visit_seq",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::abort::upload",
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::part",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some(
            "<alloc::borrow::Cow<str> as core::convert::From<serde_urlencoded::ser::key::Key>>::from",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::abort_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::put_part",
        ],
        name: "store dependency <alloc::borrow::Cow<str> as core::convert::From<serde_urlencoded::ser::key::Key>>::from",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
        ],
        platform: Platform::All,
        frame: Some(
            "<bytes::buf::reader::Reader<bytes::bytes::Bytes> as quick_xml::reader::XmlSource<&mut alloc::vec::Vec<u8>>>::read_text",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <bytes::buf::reader::Reader<bytes::bytes::Bytes> as quick_xml::reader::XmlSource<&mut alloc::vec::Vec<u8>>>::read_text",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some(
            "<bytes::buf::reader::Reader<bytes::bytes::Bytes> as quick_xml::reader::XmlSource<&mut alloc::vec::Vec<u8>>>::read_with",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <bytes::buf::reader::Reader<bytes::bytes::Bytes> as quick_xml::reader::XmlSource<&mut alloc::vec::Vec<u8>>>::read_with",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::list::Listing::page"],
        platform: Platform::All,
        frame: Some(
            "<object_store::ListResult as core::convert::TryFrom<object_store::client::s3::ListResponse>>::try_from",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
        ],
        name: "store dependency <object_store::ListResult as core::convert::TryFrom<object_store::client::s3::ListResponse>>::try_from",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::list::Listing::page"],
        platform: Platform::All,
        frame: Some("<percent_encoding::PercentDecode>::if_any"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
        ],
        name: "store dependency <percent_encoding::PercentDecode>::if_any",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::abort::upload",
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::part",
            "unityrs_batch::store::multipart::Transfer::start",
            "unityrs_batch::store::put::Request::bytes",
        ],
        platform: Platform::All,
        frame: Some("<percent_encoding::PercentEncode as core::fmt::Display>::fmt"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::ObjectStore>::put_opts",
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::abort_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::put_part",
        ],
        name: "store dependency <percent_encoding::PercentEncode as core::fmt::Display>::fmt",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::de::Deserializer<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::peek",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::de::Deserializer<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::peek",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::list::Listing::page"],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::de::Deserializer<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::start_replay",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
        ],
        name: "store dependency <quick_xml::de::Deserializer<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::start_replay",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some("<quick_xml::de::Deserializer<quick_xml::de::SliceReader>>::peek"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::de::Deserializer<quick_xml::de::SliceReader>>::peek",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
        ],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::de::XmlReader<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::drain_text",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::de::XmlReader<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::drain_text",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
        ],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::de::XmlReader<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::resolve_reference",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::de::XmlReader<quick_xml::de::IoReader<bytes::buf::reader::Reader<bytes::bytes::Bytes>>>>::resolve_reference",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::encoding::Decoder>::content"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::encoding::Decoder>::content",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some("<quick_xml::errors::serialize::DeError as serde_core::de::Error>::custom"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::errors::serialize::DeError as serde_core::de::Error>::custom",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::events::BytesEnd>::into_owned"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::events::BytesEnd>::into_owned",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::list::Listing::page"],
        platform: Platform::All,
        frame: Some("<quick_xml::events::BytesStart as core::clone::Clone>::clone"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
        ],
        name: "store dependency <quick_xml::events::BytesStart as core::clone::Clone>::clone",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::events::BytesStart>::into_owned"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::events::BytesStart>::into_owned",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::events::BytesText>::into_owned"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::events::BytesText>::into_owned",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::name::NamespaceResolver as core::default::Default>::default"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::name::NamespaceResolver as core::default::Default>::default",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::reader::state::ReaderState>::emit_start"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::reader::state::ReaderState>::emit_start",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::se::content::ContentSerializer<alloc::string::String>>::write_wrapped",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::se::content::ContentSerializer<alloc::string::String>>::write_wrapped",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::se::element::ElementSerializer<&mut alloc::string::String> as serde_core::ser::Serializer>::serialize_struct",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::se::element::ElementSerializer<&mut alloc::string::String> as serde_core::ser::Serializer>::serialize_struct",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::se::element::ElementSerializer<alloc::string::String> as serde_core::ser::Serializer>::serialize_struct",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::se::element::ElementSerializer<alloc::string::String> as serde_core::ser::Serializer>::serialize_struct",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::se::element::Struct<&mut alloc::string::String> as serde_core::ser::SerializeStruct>::end",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::se::element::Struct<&mut alloc::string::String> as serde_core::ser::SerializeStruct>::end",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some(
            "<quick_xml::se::element::Struct<alloc::string::String> as serde_core::ser::SerializeStruct>::end",
        ),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <quick_xml::se::element::Struct<alloc::string::String> as serde_core::ser::SerializeStruct>::end",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::start",
        ],
        platform: Platform::All,
        frame: Some("<quick_xml::utils::CowRef<str>>::deserialize_str"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::create_multipart",
        ],
        name: "store dependency <quick_xml::utils::CowRef<str>>::deserialize_str",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some("<serde_core::de::Unexpected as core::fmt::Display>::fmt"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency <serde_core::de::Unexpected as core::fmt::Display>::fmt",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &[
            "unityrs_batch::store::abort::upload",
            "unityrs_batch::store::list::Listing::page",
            "unityrs_batch::store::multipart::Transfer::complete",
            "unityrs_batch::store::multipart::Transfer::part",
        ],
        platform: Platform::All,
        frame: Some("form_urlencoded::append_encoded"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::abort_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::put_part",
        ],
        name: "store dependency form_urlencoded::append_encoded",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::list::Listing::page"],
        platform: Platform::All,
        frame: Some("form_urlencoded::append_pair"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated",
        ],
        name: "store dependency form_urlencoded::append_pair",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::multipart::Transfer::complete"],
        platform: Platform::All,
        frame: Some("quick_xml::se::simple_type::escape_list"),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart",
        ],
        name: "store dependency quick_xml::se::simple_type::escape_list",
    },
];
