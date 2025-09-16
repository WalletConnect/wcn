use {
    proc_macro::TokenStream,
    quote::{format_ident, quote},
    syn::{
        DeriveInput,
        GenericParam,
        Lifetime,
        Path,
        Type,
        parse_macro_input,
        parse_quote,
        visit_mut::VisitMut,
    },
};

#[proc_macro_derive(Message, attributes(message))]
pub fn derive_message(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let opts = parse_message_options(&input);

    let type_name = input.ident;

    let has_lifetime = input
        .generics
        .params
        .iter()
        .any(|param| matches!(param, GenericParam::Lifetime(_)));

    if !has_lifetime {
        return TokenStream::from(quote! {
            impl wcn_rpc::BorrowedMessage for #type_name {
                type Owned = #type_name;

                fn into_owned(self) -> Self::Owned {
                    self
                }
            }
        });
    }

    let type_name_str = type_name.to_string();

    let syn::Data::Struct(syn::DataStruct {
        fields: syn::Fields::Named(fields),
        ..
    }) = input.data
    else {
        panic!("Only named structs are supported");
    };

    let fields: Vec<_> = fields.named.into_iter().collect();

    let owned_type_name = type_name_str
        .strip_suffix("Borrowed")
        .map(|s| format_ident!("{s}"))
        .unwrap_or_else(|| format_ident!("{type_name}Owned"));

    let owned_fields = fields.iter().map(|field| {
        let field_name = &field.ident;
        let field_type = replace_named_lifetimes_with_static(field.ty.clone());

        quote! {
            pub #field_name: <#field_type as wcn_rpc::BorrowedMessage>::Owned
        }
    });

    let into_owned = fields.iter().map(|field| {
        let field_name = &field.ident;
        quote! { #field_name: wcn_rpc::BorrowedMessage::into_owned(self.#field_name) }
    });

    let default_owned_derives: Vec<Path> = vec![
        parse_quote!(Clone),
        parse_quote!(Debug),
        parse_quote!(PartialEq),
        parse_quote!(Eq),
        parse_quote!(Hash),
        parse_quote!(serde::Serialize),
        parse_quote!(serde::Deserialize),
        parse_quote!(wcn_rpc::Message),
    ];

    let owned_derives = opts.owned_derives.unwrap_or(default_owned_derives);

    let owned_derive_attr = if owned_derives.is_empty() {
        quote! {}
    } else {
        quote! { #[derive(#(#owned_derives),*)] }
    };

    TokenStream::from(quote! {
        #owned_derive_attr
        pub struct #owned_type_name {
            #(#owned_fields),*
        }

        impl<'a> wcn_rpc::BorrowedMessage for #type_name<'a> {
            type Owned = #owned_type_name;

            fn into_owned(self) -> Self::Owned {
                #owned_type_name {
                    #(#into_owned),*
                }
            }
        }
    })
}

fn replace_named_lifetimes_with_static(mut ty: Type) -> Type {
    struct LifetimeReplacer;

    impl VisitMut for LifetimeReplacer {
        fn visit_lifetime_mut(&mut self, lt: &mut Lifetime) {
            *lt = Lifetime::new("'static", lt.span());
        }
    }

    let mut replacer = LifetimeReplacer;
    replacer.visit_type_mut(&mut ty);
    ty
}

#[derive(Default)]
struct MessageOptions {
    owned_derives: Option<Vec<Path>>,
}

fn parse_message_options(input: &DeriveInput) -> MessageOptions {
    let mut opts = MessageOptions::default();

    for attr in &input.attrs {
        if !attr.path().is_ident("message") {
            continue;
        }

        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("owned_derives") {
                let mut derives = Vec::new();
                meta.parse_nested_meta(|inner| {
                    derives.push(inner.path);
                    Ok(())
                })?;
                opts.owned_derives = Some(derives);
                Ok(())
            } else {
                Ok(())
            }
        });
    }

    opts
}
