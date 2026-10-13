// Minimal hand-written subset of the Lens Overlay server protocol used by Chromium
// (third_party/lens_server_proto, BSD-style license). Only the fields Lookitan sends or reads are
// declared; prost skips every other field in a response. Enum fields are kept as i32.

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerRequest {
    #[prost(message, optional, tag = "1")]
    pub objects_request: Option<ObjectsRequest>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ObjectsRequest {
    #[prost(message, optional, tag = "1")]
    pub request_context: Option<RequestContext>,
    #[prost(message, optional, tag = "3")]
    pub image_data: Option<ImageData>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RequestContext {
    #[prost(message, optional, tag = "3")]
    pub request_id: Option<RequestId>,
    #[prost(message, optional, tag = "4")]
    pub client_context: Option<ClientContext>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RequestId {
    #[prost(uint64, tag = "1")]
    pub uuid: u64,
    #[prost(int32, tag = "2")]
    pub sequence_id: i32,
    #[prost(int32, tag = "3")]
    pub image_sequence_id: i32,
    #[prost(bytes = "vec", tag = "4")]
    pub analytics_id: Vec<u8>,
    #[prost(message, optional, tag = "6")]
    pub routing_info: Option<RoutingInfo>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RoutingInfo {
    #[prost(string, tag = "1")]
    pub server_address: String,
}

/// `lens.Platform.WEB`
pub const PLATFORM_WEB: i32 = 3;
/// `lens.LensOverlayFilterType.AUTO_FILTER`
pub const FILTER_AUTO: i32 = 7;
/// `lens.WritingDirection.WRITING_DIRECTION_TOP_TO_BOTTOM`
pub const WRITING_TOP_TO_BOTTOM: i32 = 2;

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClientContext {
    #[prost(int32, tag = "1")]
    pub platform: i32,
    #[prost(message, optional, tag = "4")]
    pub locale_context: Option<LocaleContext>,
    #[prost(message, optional, tag = "17")]
    pub client_filters: Option<AppliedFilters>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LocaleContext {
    #[prost(string, tag = "1")]
    pub language: String,
    #[prost(string, tag = "2")]
    pub region: String,
    #[prost(string, tag = "3")]
    pub time_zone: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppliedFilters {
    #[prost(message, repeated, tag = "1")]
    pub filter: Vec<AppliedFilter>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppliedFilter {
    #[prost(int32, tag = "1")]
    pub filter_type: i32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ImageData {
    #[prost(message, optional, tag = "1")]
    pub payload: Option<ImagePayload>,
    #[prost(message, optional, tag = "3")]
    pub image_metadata: Option<ImageMetadata>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ImagePayload {
    #[prost(bytes = "vec", tag = "1")]
    pub image_bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ImageMetadata {
    #[prost(int32, tag = "1")]
    pub width: i32,
    #[prost(int32, tag = "2")]
    pub height: i32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerResponse {
    #[prost(message, optional, tag = "1")]
    pub error: Option<ServerError>,
    #[prost(message, optional, tag = "2")]
    pub objects_response: Option<ObjectsResponse>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerError {
    #[prost(int32, tag = "1")]
    pub error_type: i32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ObjectsResponse {
    #[prost(message, optional, tag = "3")]
    pub text: Option<Text>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Text {
    #[prost(message, optional, tag = "1")]
    pub text_layout: Option<TextLayout>,
    #[prost(string, tag = "2")]
    pub content_language: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TextLayout {
    #[prost(message, repeated, tag = "1")]
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Paragraph {
    #[prost(message, repeated, tag = "2")]
    pub lines: Vec<Line>,
    #[prost(message, optional, tag = "3")]
    pub geometry: Option<Geometry>,
    #[prost(int32, tag = "4")]
    pub writing_direction: i32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Line {
    #[prost(message, repeated, tag = "1")]
    pub words: Vec<Word>,
    #[prost(message, optional, tag = "2")]
    pub geometry: Option<Geometry>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Word {
    #[prost(string, tag = "2")]
    pub plain_text: String,
    #[prost(string, optional, tag = "3")]
    pub text_separator: Option<String>,
    #[prost(message, optional, tag = "4")]
    pub geometry: Option<Geometry>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Geometry {
    #[prost(message, optional, tag = "1")]
    pub bounding_box: Option<CenterRotatedBox>,
}

/// Normalized (0..1) box given by its center, size and rotation in radians.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct CenterRotatedBox {
    #[prost(float, tag = "1")]
    pub center_x: f32,
    #[prost(float, tag = "2")]
    pub center_y: f32,
    #[prost(float, tag = "3")]
    pub width: f32,
    #[prost(float, tag = "4")]
    pub height: f32,
    #[prost(float, tag = "5")]
    pub rotation_z: f32,
}
