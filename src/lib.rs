use darling::FromDeriveInput;
use quote::quote;
use syn::{parse_macro_input, parse_quote};

#[proc_macro_derive(Dice, attributes(dice))]
pub fn derive_dice(raw_input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(raw_input as syn::DeriveInput);
    let type_input = TypeInput::from_derive_input(&input).map_err(syn::Error::from);

    let output = type_input
        .and_then(impl_dice)
        .unwrap_or_else(|err| err.to_compile_error());

    proc_macro::TokenStream::from(output)
}

#[derive(darling::FromDeriveInput)]
#[darling(attributes(dice))]
struct TypeInput {
    ident: syn::Ident,
    generics: syn::Generics,
    data: darling::ast::Data<VariantInput, FieldInput>,

    bound: Option<syn::punctuated::Punctuated<syn::WherePredicate, syn::token::Comma>>,
}

#[derive(darling::FromField)]
#[darling(attributes(dice))]
struct FieldInput {
    ident: Option<syn::Ident>,
    ty: syn::Type,

    die: Option<syn::Expr>,
}

#[derive(darling::FromVariant)]
#[darling(attributes(dice))]
struct VariantInput {
    ident: syn::Ident,
    fields: darling::ast::Fields<FieldInput>,

    weight: Option<u32>,
}

fn impl_dice(type_input: TypeInput) -> Result<proc_macro2::TokenStream, syn::Error> {
    let ident = clone_call_site_ident(&type_input.ident);

    let mut generics = type_input.generics;
    add_bound(type_input.bound, &mut generics);

    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    let r#type = Type::parse(type_input.data)?;
    let uses_limit = r#type.uses_limit();
    let die = r#type.die()?;

    let impl_dice = quote! {
        impl #impl_generics ::dicetest::Dice for #ident #type_generics #where_clause {
            const USES_LIMIT: bool = #uses_limit;

            fn die() -> impl ::dicetest::Die<Self> {
                #die
            }
        }
    };

    Ok(impl_dice)
}

fn add_bound(
    bound: Option<syn::punctuated::Punctuated<syn::WherePredicate, syn::token::Comma>>,
    generics: &mut syn::Generics,
) {
    if let Some(bound) = bound {
        // Add user-specified bound to where clause
        let where_clause = generics.make_where_clause();
        where_clause.predicates.extend(bound);
    } else {
        // Add type bound Dice to all type parameters
        for type_param in generics.type_params_mut() {
            type_param.bounds.push(parse_quote!(::dicetest::Dice));
        }
    }
}

struct Type {
    kind: TypeKind,
}

impl Type {
    fn parse(data: darling::ast::Data<VariantInput, FieldInput>) -> Result<Self, syn::Error> {
        let kind = match data {
            darling::ast::Data::Struct(fields_input) => {
                let fields = Fields::parse(fields_input, None);
                TypeKind::Struct(fields)
            }
            darling::ast::Data::Enum(variants_input) => {
                let variants = Variants::parse(variants_input);
                TypeKind::Enum(variants)
            }
        };

        Ok(Self { kind })
    }

    fn uses_limit(&self) -> proc_macro2::TokenStream {
        match &self.kind {
            TypeKind::Struct(fields) => fields.uses_limit(),
            TypeKind::Enum(variants) => variants.uses_limit(),
        }
    }

    fn die(&self) -> Result<proc_macro2::TokenStream, syn::Error> {
        let die = match &self.kind {
            TypeKind::Struct(fields) => fields.die(),
            TypeKind::Enum(variants) => variants.die()?,
        };

        Ok(die)
    }
}

enum TypeKind {
    Struct(Fields),
    Enum(Variants),
}

struct Field {
    pos: usize,
    ident: Option<syn::Ident>,
    ty: syn::Type,
    die: Option<syn::Expr>,
}

impl Field {
    fn parse(field_input: FieldInput, pos: usize) -> Self {
        let ident = field_input.ident.as_ref().map(clone_call_site_ident);
        let ty = field_input.ty.clone();
        let die = field_input.die;
        Field {
            pos,
            ident,
            ty,
            die,
        }
    }

    fn uses_limit(&self) -> proc_macro2::TokenStream {
        let ty = &self.ty;
        quote!(<#ty as ::dicetest::Dice>::USES_LIMIT)
    }

    fn die_name(&self) -> syn::Ident {
        call_site_ident(&format!("die_{}", self.pos))
    }

    fn die(&self) -> proc_macro2::TokenStream {
        if let Some(expr) = &self.die {
            quote!(#expr)
        } else {
            let ty = &self.ty;
            quote!(<#ty as ::dicetest::Dice>::die())
        }
    }

    fn value_name(&self) -> syn::Ident {
        call_site_ident(&format!("value_{}", self.pos))
    }
}

struct Fields {
    variant_ident: Option<syn::Ident>,
    style: darling::ast::Style,
    fields: Vec<Field>,
}

impl Fields {
    fn parse(
        fields_input: darling::ast::Fields<FieldInput>,
        variant_ident: Option<syn::Ident>,
    ) -> Self {
        let style = fields_input.style;
        let fields = fields_input
            .into_iter()
            .enumerate()
            .map(|(pos, field_input)| Field::parse(field_input, pos))
            .collect::<Vec<_>>();

        Fields {
            variant_ident,
            style,
            fields,
        }
    }

    fn uses_limit(&self) -> proc_macro2::TokenStream {
        let mut fields = self.fields.iter();
        let mut acc = if let Some(field) = fields.next() {
            field.uses_limit()
        } else {
            quote!(false)
        };
        for field in fields {
            let uses_limit = field.uses_limit();
            acc.extend(quote!(|| #uses_limit));
        }
        acc
    }

    fn uses_limit_count_impl(&self) -> proc_macro2::TokenStream {
        let mut fields = self.fields.iter();
        let mut acc = if let Some(field) = fields.next() {
            let uses_limit = field.uses_limit();
            quote!(#uses_limit as usize)
        } else {
            quote!(0usize)
        };
        for field in fields {
            let uses_limit = field.uses_limit();
            acc.extend(quote!(+ #uses_limit as usize));
        }
        acc
    }

    fn dies(&self) -> proc_macro2::TokenStream {
        let mut acc = proc_macro2::TokenStream::new();
        for field in &self.fields {
            let die_name = field.die_name();
            let die = field.die();
            acc.extend(quote!(let #die_name = #die;));
        }
        acc
    }

    fn die_values(&self) -> proc_macro2::TokenStream {
        let mut acc = proc_macro2::TokenStream::new();
        for field in &self.fields {
            let value_name = field.value_name();
            let uses_limit = field.uses_limit();
            let die_name = field.die_name();
            acc.extend(quote! {
                let #value_name = if #uses_limit {
                    fate.roll(&#die_name)
                } else {
                    fate.with_limit(::dicetest::Limit(0)).roll(&#die_name)
                };
            });
        }
        acc
    }

    fn die_values_with_limit_part(&self) -> proc_macro2::TokenStream {
        let mut acc = proc_macro2::TokenStream::new();
        for field in &self.fields {
            let value_name = field.value_name();
            let uses_limit = field.uses_limit();
            let die_name = field.die_name();
            acc.extend(quote! {
                let #value_name = if #uses_limit {
                    let limit = limit_parts.pop().unwrap();
                    fate.with_limit(limit).roll(&#die_name)
                } else {
                    fate.with_limit(::dicetest::Limit(0)).roll(&#die_name)
                };
            })
        }
        acc
    }

    fn self_with_variant(&self) -> proc_macro2::TokenStream {
        match &self.variant_ident {
            None => quote!(Self),
            Some(variant_ident) => quote!(Self::#variant_ident),
        }
    }

    fn result(&self) -> proc_macro2::TokenStream {
        let self_with_variant = self.self_with_variant();

        match self.style {
            darling::ast::Style::Struct => {
                let mut acc = proc_macro2::TokenStream::new();
                for field in &self.fields {
                    let ident = field
                        .ident
                        .as_ref()
                        .expect("Field should have identifier if Fields is of kind Named");
                    let value_name = field.value_name();
                    acc.extend(quote!(#ident: #value_name,));
                }

                quote!(#self_with_variant {
                    #acc
                })
            }
            darling::ast::Style::Tuple => {
                let mut acc = proc_macro2::TokenStream::new();
                for field in &self.fields {
                    let value_name = field.value_name();
                    acc.extend(quote!(#value_name,));
                }

                quote!(#self_with_variant(#acc))
            }
            darling::ast::Style::Unit => {
                quote!(#self_with_variant)
            }
        }
    }

    fn die(&self) -> proc_macro2::TokenStream {
        let result = self.result();

        match self.fields.len() {
            0 => {
                quote!(::dicetest::dice::from_fn(|_fate| #result))
            }
            1 => {
                let dies = self.dies();
                let die_values = self.die_values();

                quote! {
                    #dies

                    ::dicetest::dice::from_fn(move |mut fate| {
                        #die_values
                        #result
                    })
                }
            }
            _ => {
                let uses_limit_count = self.uses_limit_count_impl();
                let dies = self.dies();
                let die_values = self.die_values();
                let die_values_with_limit_part = self.die_values_with_limit_part();

                quote! {
                    let uses_limit_count = #uses_limit_count;
                    #dies

                    ::dicetest::dice::from_fn(move |mut fate| {
                        if uses_limit_count <= 1 {
                            #die_values
                            #result
                        } else {
                            let limit = fate.limit();
                            let limit_parts_die = ::dicetest::dice::split_limit_n(
                                limit,
                                uses_limit_count,
                            );
                            let mut limit_parts = fate
                                .with_limit(::dicetest::Limit(0))
                                .roll(limit_parts_die);
                            #die_values_with_limit_part
                            #result
                        }
                    })
                }
            }
        }
    }
}

struct Variant {
    pos: usize,
    weight: u64,
    fields: Fields,
}

impl Variant {
    fn parse(variant_input: VariantInput, pos: usize) -> Self {
        let weight = variant_input.weight.map_or(1, u64::from);
        let ident = clone_call_site_ident(&variant_input.ident);
        let fields = Fields::parse(variant_input.fields, Some(ident));
        Variant {
            pos,
            weight,
            fields,
        }
    }

    fn die_name(&self) -> syn::Ident {
        call_site_ident(&format!("die_{}", self.pos))
    }
}

struct Variants {
    total_weight: Option<u64>,
    variants: Vec<Variant>,
}

impl Variants {
    fn parse(variants_input: Vec<VariantInput>) -> Self {
        let mut weighted = false;

        let variants = variants_input
            .into_iter()
            .enumerate()
            .map(|(pos, variant_input)| {
                weighted = weighted || variant_input.weight.is_some();
                Variant::parse(variant_input, pos)
            })
            .collect::<Vec<_>>();

        let total_weight = weighted.then(|| variants.iter().map(|variant| variant.weight).sum());

        Variants {
            total_weight,
            variants,
        }
    }

    fn uses_limit(&self) -> proc_macro2::TokenStream {
        let mut variants = self.variants.iter();
        let mut first_uses_limit = None;

        // Search first variant with fields
        for variant in &mut variants {
            if !variant.fields.fields.is_empty() {
                first_uses_limit = Some(variant.fields.uses_limit());
                break;
            }
        }

        let mut acc = if let Some(uses_limit) = first_uses_limit {
            quote!(#uses_limit)
        } else {
            quote!(false)
        };

        // Handle remaining variants and and their fields
        for variant in variants {
            for field in &variant.fields.fields {
                let uses_limit = field.uses_limit();
                acc.extend(quote!(|| #uses_limit));
            }
        }

        acc
    }

    fn dies(&self) -> proc_macro2::TokenStream {
        let mut acc = proc_macro2::TokenStream::new();
        for variant in &self.variants {
            let die_name = variant.die_name();
            let die = variant.fields.die();
            acc.extend(quote!(let #die_name = {
                #die
            };));
        }
        acc
    }

    fn first_die_value(&self) -> proc_macro2::TokenStream {
        let variant = self
            .variants
            .first()
            .expect("First variant must be available");
        let die_name = variant.die_name();
        quote!(fate.roll(&#die_name))
    }

    fn die_value_cases(&self) -> proc_macro2::TokenStream {
        let mut acc = proc_macro2::TokenStream::new();
        for variant in &self.variants {
            let pos = variant.pos;
            let die_name = variant.die_name();
            acc.extend(quote!(#pos => fate.roll(&#die_name),));
        }
        acc
    }

    fn die_value_weighted_cases(&self) -> proc_macro2::TokenStream {
        let mut acc = proc_macro2::TokenStream::new();
        for variant in &self.variants {
            let pos = variant.pos;
            let is_last = pos + 1 == self.variants.len();
            let weight = variant.weight;
            let die_name = variant.die_name();

            acc.extend(quote! {
                if choice < #weight {
                    return fate.roll(&#die_name);
                }
            });

            if !is_last {
                acc.extend(quote!(let choice = choice - #weight;));
            }
        }
        acc
    }

    fn die(&self) -> Result<proc_macro2::TokenStream, syn::Error> {
        match self.variants.len() {
            0 => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "Cannot derive Dice for enum without variants",
            )),
            1 => {
                let dies = self.dies();
                let first_die_value = self.first_die_value();

                Ok(quote! {
                    #dies

                    ::dicetest::dice::from_fn(move |mut fate| #first_die_value)
                })
            }
            variant_count => {
                let dies = self.dies();

                if let Some(total_weight) = self.total_weight {
                    let die_value_weighted_choices = self.die_value_weighted_cases();

                    Ok(quote! {
                        #dies

                        ::dicetest::dice::from_fn(move |mut fate| {
                            let choice = fate.next_number() % #total_weight;
                            #die_value_weighted_choices
                            unreachable!();
                        })
                    })
                } else {
                    let die_value_cases = self.die_value_cases();

                    Ok(quote! {
                        #dies

                        ::dicetest::dice::from_fn(move |mut fate| {
                            let variant = fate.next_number() as usize % #variant_count;
                            match variant {
                                #die_value_cases
                                _ => unreachable!(),
                            }
                        })
                    })
                }
            }
        }
    }
}

fn call_site_ident(string: &str) -> syn::Ident {
    syn::Ident::new(string, proc_macro2::Span::call_site())
}

fn clone_call_site_ident(ident: &syn::Ident) -> syn::Ident {
    let mut cloned = ident.clone();
    cloned.set_span(proc_macro2::Span::call_site());
    cloned
}
