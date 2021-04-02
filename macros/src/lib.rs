use ::syn::spanned::Spanned as _;
use proc_macro::TokenStream;

fn unit_type() -> syn::Type {
    syn::Type::Tuple(syn::TypeTuple {
        paren_token: syn::token::Paren {
            span: proc_macro2::Span::call_site(),
        },
        elems: syn::punctuated::Punctuated::new(),
    })
}

#[derive(Debug, Default)]
struct Aliases(Vec<String>);

impl darling::FromMeta for Aliases {
    fn from_list(items: &[::syn::NestedMeta]) -> darling::Result<Self> {
        items
            .iter()
            .map(|item| String::from_nested_meta(item))
            .collect::<darling::Result<Vec<String>>>()
            .map(Self)
    }
}

// #[derive(Debug)]
#[derive(Debug, darling::FromMeta)]
struct Args {
    #[darling(default)]
    aliases: Aliases,
    #[darling(default)]
    track_edits: bool,
    #[darling(default)]
    broadcast_typing: Option<bool>,
}

fn extract_help_from_docstrings(attrs: &[syn::Attribute]) -> (Option<String>, Option<String>) {
    let mut doc_lines = String::new();
    for attr in attrs {
        if attr.path == quote::format_ident!("doc").into() {
            for token in attr.tokens.clone() {
                if let proc_macro2::TokenTree::Literal(literal) = token {
                    // Someone pls tell me how to extract a literal value properly
                    doc_lines += literal
                        .to_string()
                        .trim_start_matches('"')
                        .trim_end_matches('"')
                        .trim();
                    doc_lines += "\n";
                }
            }
        }
    }

    let mut paragraphs = doc_lines.split("\n\n");

    let inline_help = paragraphs.next().map(|s| s.replace("\n", " "));

    let mut multiline_help = String::new();
    for paragraph in paragraphs {
        for line in paragraph.split('\n') {
            multiline_help += line;
            multiline_help += " ";
        }
        multiline_help += "\n\n";
    }
    let multiline_help = if multiline_help.is_empty() {
        None
    } else {
        Some(multiline_help)
    };

    (inline_help, multiline_help)
}

struct AllLifetimesToStatic;
impl syn::fold::Fold for AllLifetimesToStatic {
    fn fold_lifetime(&mut self, _: syn::Lifetime) -> syn::Lifetime {
        syn::Lifetime {
            apostrophe: proc_macro2::Span::call_site(),
            ident: syn::Ident::new("static", proc_macro2::Span::call_site()),
        }
    }
}

// Convert None => None and Some(T) => Some(T)
fn wrap_option<T: quote::ToTokens>(literal: Option<T>) -> syn::Expr {
    match literal {
        Some(literal) => syn::parse_quote! { Some(#literal) },
        None => syn::parse_quote! { None },
    }
}

fn command_inner(
    args: Args,
    function: syn::ItemFn,
) -> Result<TokenStream, (proc_macro2::Span, &'static str)> {
    let ctx_type = match function.sig.inputs.first() {
        Some(syn::FnArg::Typed(syn::PatType { ty, .. })) => &**ty,
        _ => return Err((function.sig.span(), "expected a Context parameter")),
    };

    let mut arg_name = Vec::new();
    let mut arg_type = Vec::new();
    let mut arg_attrs = Vec::new();
    for command_arg in function.sig.inputs.iter().skip(1) {
        let pattern = match command_arg {
            syn::FnArg::Typed(x) => x,
            syn::FnArg::Receiver(r) => return Err((r.span(), "self argument is invalid here")),
        };

        arg_name.push(&pattern.pat);
        arg_type.push(&pattern.ty);
        arg_attrs.push(&pattern.attrs);
    }

    let return_type = match function.sig.output {
        syn::ReturnType::Default => unit_type(),
        syn::ReturnType::Type(_, ty) => *ty,
    };

    let (description, explanation) = extract_help_from_docstrings(&function.attrs);
    let description = wrap_option(description);
    let explanation = wrap_option(explanation);

    // Needed because we're not allowed to have lifetimes in the hacky use case below
    let ctx_type_with_static = syn::fold::fold_type(&mut AllLifetimesToStatic, ctx_type.clone());

    let function_name = function.sig.ident.clone();
    let function_visibility = function.vis;
    let function_body = function.block;
    let track_edits = args.track_edits;
    let broadcast_typing = wrap_option(args.broadcast_typing);
    let aliases = args.aliases.0;
    Ok(TokenStream::from(quote::quote! {
        #function_visibility fn #function_name() -> ::poise::Command<
            <#ctx_type_with_static as poise::_GetGenerics>::U,
            <#ctx_type_with_static as poise::_GetGenerics>::E,
        > {
            fn inner(ctx: #ctx_type, args: &str) -> #return_type {
                let ( #( #arg_name ),* ) = ::poise::parse_args!(args => #(
                    #( #arg_attrs )* (#arg_type)
                ),* )?;

                #function_body
            }

            ::poise::Command {
                name: stringify!(#function_name),
                action: inner,
                options: ::poise::CommandOptions {
                    track_edits: #track_edits,
                    broadcast_typing: #broadcast_typing,
                    aliases: &[ #( #aliases ),* ],
                    description: #description,
                    explanation: #explanation,

                    check: None,
                    on_error: None,
                }
            }
        }
    }))
}

#[proc_macro_attribute]
pub fn command(args: TokenStream, function: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(args as syn::AttributeArgs);
    let args = match <Args as darling::FromMeta>::from_list(&args) {
        Ok(x) => x,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let function = syn::parse_macro_input!(function as syn::ItemFn);

    match command_inner(args, function) {
        // Ok(x) => panic!("{}", x),
        Ok(x) => x,
        Err((span, msg)) => syn::Error::new(span, msg).into_compile_error().into(),
    }
}