use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, Pat, ReturnType, Type};

/// `#[tool]` proc-macro: generates a `Tool` impl from an async fn.
///
/// ## Usage
/// ```ignore
/// #[tool]
/// /// Search the web for information.
/// async fn web_search(query: String) -> String {
///     // ... implementation
/// }
/// ```
///
/// This generates a struct `WebSearch` that implements the `Tool` trait.
/// The doc comment becomes the `description()`.
/// Each parameter becomes a JSON Schema property.
#[proc_macro_attribute]
pub fn tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    match tool_impl(func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn tool_impl(func: ItemFn) -> syn::Result<TokenStream2> {
    let func_name = &func.sig.ident;
    let func_name_str = func_name.to_string();

    // Struct name: snake_case → PascalCase
    let struct_name = to_pascal_case(&func_name_str);
    let struct_ident = syn::Ident::new(&struct_name, func_name.span());

    // Extract doc comments as description
    let description = extract_doc_comments(&func.attrs);

    // Extract parameters (skip &self, context: ToolContext, etc.)
    let params = extract_params(&func.sig.inputs)?;

    // Build JSON Schema properties
    let schema_props = build_schema_props(&params);
    let required_fields: Vec<&str> = params.iter().map(|(name, _)| name.as_str()).collect();

    // Build argument extraction in execute()
    let arg_extractions = build_arg_extractions(&params);
    let arg_idents: Vec<syn::Ident> = params
        .iter()
        .map(|(name, _)| syn::Ident::new(name, proc_macro2::Span::call_site()))
        .collect();

    let vis = &func.vis;
    let block = &func.block;
    let ret_type = match &func.sig.output {
        ReturnType::Default => quote! { String },
        ReturnType::Type(_, t) => quote! { #t },
    };

    let required_json = required_fields.iter().map(|s| quote! { #s });

    Ok(quote! {
        #[derive(Debug, Clone)]
        #vis struct #struct_ident;

        impl #struct_ident {
            pub fn new() -> Self { Self }
        }

        impl ::remi_agentloop::tool::Tool for #struct_ident {
            fn name(&self) -> &str {
                #func_name_str
            }

            fn description(&self) -> &str {
                #description
            }

            fn parameters_schema(&self) -> ::serde_json::Value {
                ::serde_json::json!({
                    "type": "object",
                    "properties": {
                        #(#schema_props),*
                    },
                    "required": [#(#required_json),*]
                })
            }

            fn execute(
                &self,
                arguments: ::serde_json::Value,
            ) -> impl ::std::future::Future<
                Output = ::std::result::Result<
                    ::remi_agentloop::tool::ToolResult<
                        impl ::futures::Stream<Item = ::remi_agentloop::tool::ToolOutput>
                    >,
                    ::remi_agentloop::error::AgentError
                >
            > {
                async move {
                    #(#arg_extractions)*
                    let result: #ret_type = {
                        let #(#arg_idents),* = #(#arg_idents),*;
                        #block
                    };
                    let result_str = result.to_string();
                    Ok(::remi_agentloop::tool::ToolResult::Output(
                        ::async_stream::stream! {
                            yield ::remi_agentloop::tool::ToolOutput::Result(result_str);
                        }
                    ))
                }
            }
        }
    })
}

fn extract_doc_comments(attrs: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    lines.push(s.value().trim().to_string());
                }
            }
        }
    }
    lines.join(" ")
}

fn extract_params(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> syn::Result<Vec<(String, String)>> {
    let mut params = Vec::new();
    for input in inputs {
        match input {
            FnArg::Receiver(_) => {} // skip self
            FnArg::Typed(pt) => {
                let name = match pt.pat.as_ref() {
                    Pat::Ident(pi) => pi.ident.to_string(),
                    _ => continue,
                };
                let type_str = type_to_json_schema_type(pt.ty.as_ref());
                params.push((name, type_str));
            }
        }
    }
    Ok(params)
}

fn type_to_json_schema_type(ty: &Type) -> String {
    let ts = quote! { #ty }.to_string();
    let ts = ts.replace(" ", "");
    match ts.as_str() {
        "String" | "&str" => "string".to_string(),
        "i64" | "i32" | "i16" | "i8" | "u64" | "u32" | "u16" | "u8" | "usize" | "isize" => {
            "integer".to_string()
        }
        "f64" | "f32" => "number".to_string(),
        "bool" => "boolean".to_string(),
        _ => "string".to_string(),
    }
}

fn build_schema_props(params: &[(String, String)]) -> Vec<TokenStream2> {
    params
        .iter()
        .map(|(name, ty)| {
            quote! {
                #name: { "type": #ty }
            }
        })
        .collect()
}

fn build_arg_extractions(params: &[(String, String)]) -> Vec<TokenStream2> {
    params
        .iter()
        .map(|(name, ty)| {
            let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
            let extraction = match ty.as_str() {
                "integer" => quote! {
                    let #ident: i64 = arguments[#name].as_i64()
                        .ok_or_else(|| ::remi_agentloop::error::AgentError::ToolExecution {
                            tool_name: stringify!(#ident).to_string(),
                            message: format!("missing or invalid integer argument: {}", #name),
                        })?;
                },
                "number" => quote! {
                    let #ident: f64 = arguments[#name].as_f64()
                        .ok_or_else(|| ::remi_agentloop::error::AgentError::ToolExecution {
                            tool_name: stringify!(#ident).to_string(),
                            message: format!("missing or invalid number argument: {}", #name),
                        })?;
                },
                "boolean" => quote! {
                    let #ident: bool = arguments[#name].as_bool()
                        .ok_or_else(|| ::remi_agentloop::error::AgentError::ToolExecution {
                            tool_name: stringify!(#ident).to_string(),
                            message: format!("missing or invalid boolean argument: {}", #name),
                        })?;
                },
                _ => quote! {
                    let #ident: String = arguments[#name].as_str()
                        .ok_or_else(|| ::remi_agentloop::error::AgentError::ToolExecution {
                            tool_name: stringify!(#ident).to_string(),
                            message: format!("missing or invalid string argument: {}", #name),
                        })?
                        .to_string();
                },
            };
            extraction
        })
        .collect()
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}
