use heck::ToPascalCase;
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    FnArg, Ident, ItemForeignMod, Meta, Pat, PatType, ReturnType, Token, Type,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    token::Comma,
};

/// Represents service-level attributes that can be inherited by methods
#[derive(Default)]
struct ServiceAttr {
    request: MethodAttr,
    response: MethodAttr,
}

/// Represents a method attribute for configuring request/response behavior
#[derive(Clone, Default)]
struct MethodAttr {
    stream: Option<bool>, // None = not specified (will default to true), Some(bool) = explicit setting
    header: Option<Type>,
    with_peer_addr: Option<bool>, // None = inherit, Some(bool) = explicit setting
    raw: Option<bool>,            // None = inherit, Some(bool) = explicit setting for raw responses
    metrics: Vec<TokenStream>,    // Custom metrics for this request/response
}

impl MethodAttr {
    fn header_tokens(&self) -> TokenStream {
        if let Some(ty) = &self.header {
            let ty = quote!(#ty).to_string();
            quote!(Some(#ty.to_string()))
        } else {
            quote!(None)
        }
    }

    fn is_stream(&self) -> bool {
        self.stream.unwrap_or(false)
    }

    /// Merge attributes from another MethodAttr, only overriding values that are explicitly set
    fn merge(&mut self, other: MethodAttr) {
        if let Some(stream) = other.stream {
            self.stream = Some(stream);
        }
        if let Some(header) = other.header {
            self.header = Some(header);
        }
        if let Some(with_peer_addr) = other.with_peer_addr {
            self.with_peer_addr = Some(with_peer_addr);
        }
        if let Some(raw) = other.raw {
            self.raw = Some(raw);
        }
        // Extend metrics instead of replacing them
        self.metrics.extend(other.metrics);
    }

    fn has_peer_addr(&self) -> bool {
        self.with_peer_addr.unwrap_or(false)
    }

    fn is_raw(&self) -> bool {
        self.raw.unwrap_or(false)
    }
}

impl Parse for MethodAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut attr = MethodAttr::default();

        let metas = input.parse_terminated(Meta::parse, Token![,])?;

        macro_rules! parse_bool_param {
            ($nv:ident) => {
                Some(match &$nv.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Bool(b),
                        ..
                    }) => b.value,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            $nv.value,
                            "value must be a boolean literal",
                        ));
                    }
                })
            };
        }

        for meta in metas {
            match meta {
                // Support bare "stream" as true
                Meta::Path(path) if path.is_ident("stream") => {
                    attr.stream = Some(true);
                }
                // Support stream = true/false
                Meta::NameValue(nv) if nv.path.is_ident("stream") => {
                    attr.stream = parse_bool_param!(nv);
                }
                // Support bare "with_peer_addr" as true
                Meta::Path(path) if path.is_ident("with_peer_addr") => {
                    attr.with_peer_addr = Some(true);
                }
                // Support with_peer_addr = true/false
                Meta::NameValue(nv) if nv.path.is_ident("with_peer_addr") => {
                    attr.with_peer_addr = parse_bool_param!(nv);
                }
                // Support bare "raw" as true
                Meta::Path(path) if path.is_ident("raw") => {
                    attr.raw = Some(true);
                }
                // Support raw = true/false
                Meta::NameValue(nv) if nv.path.is_ident("raw") => {
                    attr.raw = parse_bool_param!(nv);
                }
                Meta::NameValue(nv) if nv.path.is_ident("header") => {
                    let ty = match &nv.value {
                        syn::Expr::Path(p) => {
                            syn::parse_quote!(#p)
                        }
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(s),
                            ..
                        }) => syn::parse_str(&s.value())?,
                        _ => {
                            return Err(syn::Error::new_spanned(
                                nv.value,
                                "header value must be a path or string literal",
                            ));
                        }
                    };
                    attr.header = Some(ty);
                }
                // Handle metric function calls like counter(...), measure(...), etc.
                Meta::List(list) => {
                    if let Some(metric) = crate::event::parse_meta_list(&list)? {
                        attr.metrics.push(metric);
                    } else {
                        return Err(syn::Error::new_spanned(list, "unknown attribute parameter"));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(meta, "unknown attribute parameter"));
                }
            }
        }

        Ok(attr)
    }
}

// Define a struct to hold parsed method information
struct MethodInfo {
    name: Ident,
    docs: Vec<String>,
    inputs: Punctuated<FnArg, Comma>,
    output: TokenStream,
    unidirectional: bool,
    unreliable: bool,
    request: MethodAttr,
    response: MethodAttr,
    level: String,
    /// The transport-level send priority variant name (e.g. `"High"`), as
    /// written in `#[priority = High]`. Defaults to `"Medium"`.
    priority: String,
}

/// The valid `Priority` variant names, mirroring
/// `upstream_core::service::Priority`. Used to validate `#[priority = ...]` at
/// macro-expansion time so typos fail to compile with a clear message.
const PRIORITY_VARIANTS: &[&str] = &[
    "Highest",
    "VeryHigh",
    "High",
    "Medium",
    "MediumLow",
    "Low",
    "VeryLow",
    "Lowest",
];

impl MethodInfo {
    fn new(method: syn::ForeignItemFn, service_attr: &ServiceAttr) -> syn::Result<Self> {
        let name = method.sig.ident;
        let inputs = method.sig.inputs;

        let output = match &method.sig.output {
            ReturnType::Default => quote!(()),
            ReturnType::Type(_, ty) => quote!(#ty),
        };

        // Start with service-level attributes
        let mut request = service_attr.request.clone();
        let mut response = service_attr.response.clone();
        let mut unidirectional = false;
        let mut unreliable = false;
        let mut docs = vec![];
        let mut level = String::from("trace");
        let mut priority = String::from("Medium");

        // Process all attributes and validate them
        for attr in &method.attrs {
            if attr.path().is_ident("request") {
                let attrs = attr.parse_args()?;
                request.merge(attrs);
            } else if attr.path().is_ident("response") {
                let attrs = attr.parse_args()?;
                response.merge(attrs);
            } else if attr.path().is_ident("unidirectional") {
                unidirectional = true;
            } else if attr.path().is_ident("unreliable") {
                unreliable = true;
            } else if attr.path().is_ident("doc") {
                docs.push(parse_str_attr(attr)?);
            } else if attr.path().is_ident("level") {
                level = parse_str_attr(attr)?;
            } else if attr.path().is_ident("priority") {
                priority = parse_priority_attr(attr)?;
            } else {
                // error on anything else
                return Err(syn::Error::new_spanned(
                    attr,
                    format!("unknown attribute `{}`", attr.path().get_ident().unwrap()),
                ));
            }
        }

        Ok(Self {
            name,
            docs,
            inputs,
            output,
            unidirectional,
            unreliable,
            request,
            response,
            level,
            priority,
        })
    }

    fn generate_method_def(&self) -> TokenStream {
        let name = &self.name;
        let docs = &self.docs;
        let output = &self.output;

        let unidirectional = if self.unidirectional {
            quote!({
                upstream_codegen::macro_support::assert_unidirectional_output::<#output>();
                true
            })
        } else {
            quote!(false)
        };

        let unreliable = self.unreliable;
        let stream_request = self.request.is_stream();
        let stream_response = self.response.is_stream();
        let peer_addr = self.request.has_peer_addr();
        let raw_response = self.response.is_raw();

        let request_header = self.request.header_tokens();
        let response_header = self.response.header_tokens();
        let priority = &self.priority;

        let inputs = self.inputs.iter().filter_map(|arg| {
            if let FnArg::Typed(PatType { pat, ty, .. }) = arg {
                if let Pat::Ident(pat_ident) = &**pat {
                    Some(quote!(upstream_codegen::service::Input {
                        name: stringify!(#pat_ident).to_string(),
                        description: vec![],
                        ty: stringify!(#ty).to_string(),
                    }))
                } else {
                    None
                }
            } else {
                None
            }
        });

        quote! {
            upstream_codegen::service::Method {
                name: stringify!(#name).to_string(),
                description: vec![#(#docs.to_string()),*],
                inputs: vec![
                    #(#inputs),*
                ],
                output: upstream_codegen::service::Output {
                    description: String::new(),
                    ty: stringify!(#output).to_string(),
                },
                unidirectional: #unidirectional,
                unreliable: #unreliable,
                stream_request: #stream_request,
                stream_response: #stream_response,
                request_header: #request_header,
                response_header: #response_header,
                peer_addr: #peer_addr,
                raw_response: #raw_response,
                priority: #priority.to_string(),
            }
        }
    }

    fn generate_span_event(&self, service: &Ident, side: &str) -> TokenStream {
        let name = &self.name;
        let level = &self.level;
        let method_name = format!("{service}:{side}:{name}");
        let event_name = format!("{service}.{side}.{name}");
        let struct_name = event_name.to_pascal_case();

        let pascal_name = Ident::new(&self.name.to_string().to_pascal_case(), Span::call_site());
        let req_ty = quote!(service::#service::schema::request::#pascal_name);

        // For raw responses, use usize as the response type for spans
        let res_ty = if self.response.is_raw() {
            quote!(usize)
        } else {
            quote!(service::#service::schema::response::#pascal_name)
        };

        // Fold the response header into the span's result type for UNARY
        // methods with a response header. The unary dispatch path requires
        // `Span: Unary<Req, (ResH, Res)>` (consumer `send_with_response_header`)
        // and `Span: FullResponse<(ResH, Res)>` (provider `write_with_header`),
        // so the span result must be the `(header, body)` tuple the method
        // actually returns. Streaming-response methods keep the body-only type:
        // their with-header dispatch (`write_stream_with_header` /
        // `send_stream_response_with_response_header`) carries the header out of
        // band and bounds the span only by `'static + Send`.
        let res_ty = match (self.response.header.is_some(), self.response.is_stream()) {
            (true, false) => {
                quote!((service::#service::schema::response::header::#pascal_name, #res_ty))
            }
            _ => res_ty,
        };

        // Request-header handling (unchanged): the provider span folds in the
        // request header. (The response-header fold above is the fix for the
        // previously-unhandled unary response-header case.)
        let req_ty = match (side, self.request.header.is_some()) {
            ("provider", true) => {
                quote!((service::#service::schema::request::header::#pascal_name, #req_ty))
            }
            _ => req_ty,
        };

        let side = Ident::new(side, proc_macro2::Span::call_site());

        // Determine the appropriate span type based on method attributes
        let span_type = match (
            self.request.is_stream(),
            self.response.is_stream(),
            self.unidirectional,
        ) {
            (_, _, true) => quote!(Unidirectional),
            (true, true, _) => quote!(bidi::Stream),
            (true, false, _) => quote!(StreamRequest),
            (false, true, _) => quote!(StreamResponse),
            (false, false, _) => quote!(Unary),
        };

        let response_type = if self.unidirectional {
            quote!(None)
        } else {
            quote!(Some(|| upstream_codegen::macro_support::quote!(#res_ty)))
        };

        let request_metrics = &self.request.metrics;
        let response_metrics = &self.response.metrics;

        quote! {
            upstream_codegen::events::EventMetadata {
                name: #event_name,
                struct_name: #struct_name,
                docs: &[],
                // fields are emitted on sub-span events
                fields: &[],
                generics: || upstream_codegen::macro_support::quote!(),
                level: #level,
                event_type: upstream_codegen::events::EventType::MethodSpan {
                    method_name: #method_name,
                    span_type: || upstream_codegen::macro_support::quote!(#side::span::#span_type),
                    request_type: || upstream_codegen::macro_support::quote!(#req_ty),
                    response_type: #response_type,
                    request_metrics: &[#(#request_metrics),*],
                    response_metrics: &[#(#response_metrics),*],
                },
                backbeat_skip: false,
            }
        }
    }
}

pub(crate) fn service(name: Ident, input: ItemForeignMod) -> syn::Result<TokenStream> {
    // Parse service-level attributes
    let mut service_attr = ServiceAttr::default();
    for attr in &input.attrs {
        if attr.path().is_ident("request") {
            service_attr.request = attr.parse_args()?;
        } else if attr.path().is_ident("response") {
            service_attr.response = attr.parse_args()?;
        }
    }

    // Parse all methods and create MethodInfo structs
    let mut method_infos = vec![];
    let mut errors = vec![];

    for item in input.items {
        if let syn::ForeignItem::Fn(method) = item {
            match MethodInfo::new(method, &service_attr) {
                Ok(info) => method_infos.push(info),
                Err(e) => errors.push(e),
            }
        }
    }

    // If there were any errors, combine them into a single error
    if !errors.is_empty() {
        let mut errors = errors.into_iter();
        let mut combined_error = errors.next().unwrap();
        for error in errors {
            combined_error.combine(error);
        }
        return Err(combined_error);
    }

    // Generate method definitions
    let method_defs = method_infos.iter().map(|info| info.generate_method_def());

    // Generate events
    let events = method_infos.iter().flat_map(|info| {
        vec![
            info.generate_span_event(&name, "consumer"),
            info.generate_span_event(&name, "provider"),
        ]
    });

    let name_str = name.to_string();

    Ok(quote! {
        pub fn #name() -> upstream_codegen::service::Service {
            let mut service = upstream_codegen::service::Service::new(#name_str);
            #(
                service = service.with(#method_defs);
            )*
            #(
                service = service.with_event(#events);
            )*
            service
        }
    })
}

/// Parse `#[priority = High]` into the validated variant name `"High"`.
///
/// Accepts the variant as a bare path (`#[priority = High]`) or a string
/// literal (`#[priority = "High"]`). Rejects anything that is not one of the
/// known [`PRIORITY_VARIANTS`] so typos fail at macro-expansion time.
fn parse_priority_attr(attr: &syn::Attribute) -> syn::Result<String> {
    let syn::Meta::NameValue(nv) = &attr.meta else {
        return Err(syn::Error::new_spanned(
            attr,
            "priority attribute must be a name-value pair, e.g. `#[priority = High]`",
        ));
    };

    let variant = match &nv.value {
        syn::Expr::Path(p) => p
            .path
            .get_ident()
            .map(|ident| ident.to_string())
            .ok_or_else(|| {
                syn::Error::new_spanned(&nv.value, "priority value must be a single identifier")
            })?,
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => s.value(),
        _ => {
            return Err(syn::Error::new_spanned(
                &nv.value,
                "priority value must be a path or string literal",
            ));
        }
    };

    if !PRIORITY_VARIANTS.contains(&variant.as_str()) {
        return Err(syn::Error::new_spanned(
            &nv.value,
            format!(
                "unknown priority `{variant}`; expected one of: {}",
                PRIORITY_VARIANTS.join(", ")
            ),
        ));
    }

    Ok(variant)
}

fn parse_str_attr(attr: &syn::Attribute) -> syn::Result<String> {
    if let syn::Meta::NameValue(nv) = &attr.meta {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = &nv.value
        {
            Ok(s.value())
        } else {
            Err(syn::Error::new_spanned(
                &nv.value,
                "doc attribute value must be a string literal",
            ))
        }
    } else {
        Err(syn::Error::new_spanned(
            attr,
            "doc attribute must be a name-value pair",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn test_method_attr_parsing() {
        // Test with_peer_addr attribute
        let attr: MethodAttr = parse_quote!(with_peer_addr);
        assert_eq!(attr.with_peer_addr, Some(true));
        assert!(!attr.is_stream());
        assert!(attr.header.is_none());

        // Test stream attribute
        let attr: MethodAttr = parse_quote!(stream);
        assert!(attr.is_stream());
        assert!(attr.header.is_none());

        // Test header attribute
        let attr: MethodAttr = parse_quote!(header = "crate::AuthHeader");
        assert!(!attr.is_stream());
        assert!(attr.header.is_some());

        // Test both attributes
        let attr: MethodAttr = parse_quote!(stream, header = "crate::AuthHeader");
        assert!(attr.is_stream());
        assert!(attr.header.is_some());
    }

    #[test]
    fn test_service_macro() {
        let name = Ident::new("test_service", proc_macro2::Span::call_site());
        let input = parse_quote! {
            #[request(stream = true, with_peer_addr)]
            #[response(header = "crate::GlobalHeader")]
            extern "RPC" {
                /// Get a value by key
                fn get(key: Vec<u8>) -> Option<Vec<u8>>;

                // Inherits request stream=true, overrides response header
                #[response(header = "crate::AuthHeader")]
                fn stream_with_header(data: Vec<u8>) -> Vec<u8>;

                // Explicitly disables request streaming, inherits response header
                #[request(stream = false)]
                fn response_stream_with_header() -> Vec<u8>;
            }
        };

        let output = service(name, input).unwrap();
        let output_str = output.to_string();

        println!("OUTPUT:\n{output_str}\n");
        // Verify fully-qualified paths
        assert!(output_str.contains("upstream_codegen :: service :: Service"));
        assert!(output_str.contains("upstream_codegen :: service :: Method"));
        assert!(output_str.contains("upstream_codegen :: service :: Input"));
        assert!(output_str.contains("upstream_codegen :: service :: Output"));
        assert!(output_str.contains("upstream_codegen :: events :: EventMetadata"));
        assert!(output_str.contains("upstream_codegen :: events :: EventType"));

        // Verify service name is used
        assert!(output_str.contains("\"test_service\""));

        // Verify service-level attributes are inherited
        assert!(output_str.contains("stream_request : true")); // From service-level request(stream = true)
        assert!(output_str.contains("stream_request : false")); // From method-level request(stream = false)
        assert!(output_str.contains("GlobalHeader")); // From service-level
        assert!(output_str.contains("AuthHeader")); // From method-level override
        assert!(output_str.contains("peer_addr : true")); // From service-level request(with_peer_addr)

        // Test explicit with_peer_addr = false
        let attr: MethodAttr = parse_quote!(with_peer_addr = false);
        assert_eq!(attr.with_peer_addr, Some(false));
    }

    #[test]
    fn test_priority_attribute() {
        let name = Ident::new("test_service", proc_macro2::Span::call_site());
        let input = parse_quote! {
            extern "RPC" {
                // Defaults to Medium when unspecified.
                fn no_priority() -> Vec<u8>;

                // Bare path form.
                #[priority = High]
                fn high(data: Vec<u8>) -> Vec<u8>;

                // String literal form.
                #[priority = "Lowest"]
                fn lowest() -> Vec<u8>;
            }
        };

        let output = service(name, input).unwrap();
        let output_str = output.to_string();

        // Default is emitted as Medium.
        assert!(output_str.contains("priority : \"Medium\" . to_string ()"));
        // Explicit variants are carried through verbatim.
        assert!(output_str.contains("priority : \"High\" . to_string ()"));
        assert!(output_str.contains("priority : \"Lowest\" . to_string ()"));
    }

    #[test]
    fn test_priority_unknown_variant_errors() {
        let name = Ident::new("test_service", proc_macro2::Span::call_site());
        let input = parse_quote! {
            extern "RPC" {
                #[priority = Bogus]
                fn bad() -> Vec<u8>;
            }
        };

        let err = service(name, input).unwrap_err();
        assert!(err.to_string().contains("unknown priority `Bogus`"));
    }
}
