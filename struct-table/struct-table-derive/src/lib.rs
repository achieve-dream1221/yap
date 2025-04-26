use deluxe::ParseMetaItem;
use heck::ToTitleCase;
use proc_macro2_diagnostics::SpanDiagnosticExt;
use quote::quote;
use syn::DeriveInput;

extern crate proc_macro;

#[proc_macro_derive(StructTable, attributes(table))]
pub fn struct_table(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match struct_table_inner(input.into()) {
        Ok(token_stream) => token_stream.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn struct_table_inner(input: proc_macro2::TokenStream) -> deluxe::Result<proc_macro2::TokenStream> {
    let mut ast: DeriveInput = syn::parse2(input)?;

    // panic!("{ast:#?}");

    // Extracting the 'struct-global' attributes
    let struct_attrs: StructTableAttributes = deluxe::extract_attributes(&mut ast)?;
    // Vec of each processed field, in order of declaration
    let field_attrs: Vec<StructField> = extract_field_attrs(&mut ast)?;

    // Using the given #[table(rename = "Name")] if provided,
    // otherwise converting the ident to Title Case
    let field_human_names: Vec<String> = field_attrs
        .iter()
        .map(|f| {
            let mut display_name = f
                .rename
                .clone()
                .unwrap_or(f.ident.to_string().to_title_case());

            display_name.push(':');

            display_name
        })
        .collect();

    // Building the Expressions {} that will output the field's current value as a String
    let field_string_values: Vec<_> = field_attrs
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let field_to_string = if let Some(overrides) = &f.display_override {
                // If an override for Display was given

                // Bools use simpler logic, flipping between [0] and [1]
                // in [true, false] order
                if f.is_bool {
                    quote! {
                        {
                            let overrides = const {
                                let overrides = #overrides;
                                // These extra assertions are for when a user passes in an Expr/Ident
                                // instead of an array directly.
                                // I still want to make sure at compile-time that the lengths
                                // of the arrays are what I expect even if I can't check the
                                // given array myself.
                                assert!(overrides.len() == 2,
                                "bool variant labels should have exactly two labels");
                                overrides
                            };
                            // To avoid having #overrides placed in more than once, we do the
                            // length assertion without confirming the type, and then
                            // we coerce the [&str; len] to a &[&str] here.
                            let overrides_ref: &[&'static str] = overrides.as_ref();


                            let label_index: usize = if self.#ident { 1 } else { 0 };

                            overrides_ref[label_index]
                        }
                    }
                } else {
                    // Getting the overridden label with the same index of the field's value
                    let variants = f
                        .values_to_cycle
                        .as_ref()
                        .expect("expected list of values to cycle through");
                    quote! {
                        {
                            let (overrides, variants) = const {
                                let overrides = #overrides;
                                let variants = #variants;
                                assert!(overrides.len() == variants.len(),
                                "variant labels length should match cycled values length");
                                (overrides, variants)
                            };
                            let overrides_ref: &[&'static str] = overrides.as_ref();

                            let current_position: usize = variants.iter().position(|v: &_| v == &self.#ident ).expect("current variant not in given list");

                            overrides_ref[current_position]
                        }
                    }
                }
            } else {
                // If no override was given, attempt to convert the field's value to a String
                quote! {
                    self.#ident.to_string()
                }
            };

            field_to_string
        })
        .collect();

    // let string: String = field_attrs.iter().map(|(i, a)| i.to_string()).collect();
    // panic!("{string}");

    // for (field, attr) in field_attrs {

    // }

    let docstrings: Vec<_> = field_attrs.iter().map(|a| a.doc.clone()).collect();

    let inner_wrap = |no_wrap: bool| -> proc_macro2::TokenStream {
        if !no_wrap {
            quote! {
                let new_index = if next {
                    if current_position >= last_index {
                        0
                    } else {
                        current_position + 1
                    }
                } else {
                    if current_position == 0 {
                        last_index
                    } else {
                        current_position - 1
                    }
                };
            }
        } else {
            quote! {
                let new_index = if next {
                    if current_position >= last_index {
                        last_index
                    } else {
                        current_position + 1
                    }
                } else {
                    if current_position == 0 {
                        0
                    } else {
                        current_position - 1
                    }
                };
            }
        }
    };

    // Vec of field indices for match arms + the logic to cycle between values
    let (field_indices, field_arms): (Vec<_>, Vec<_>) = field_attrs
        .into_iter()
        .map(|a| {
            let ident = &a.ident;
            // Bools just always flip
            // (regardless of no_wrap, for now?)
            if a.is_bool {
                quote! {
                    self.#ident = !self.#ident;
                }
            } else {
                let variants = a
                    .values_to_cycle
                    .expect("expected list of values to cycle through");

                let inner_wrap_logic = inner_wrap(a.no_inner_wrap);

                quote! {
                    let variants: &[_] = #variants.as_ref();

                    let current_position: usize = variants.iter().position(|v: &_| v == &self.#ident ).expect("current variant not in given list");

                    let last_index = variants.len() - 1;

                    #inner_wrap_logic

                    self.#ident = variants[new_index].clone().into();
                }
            }
        })
        .enumerate()
        .unzip();

    let ident = &ast.ident;

    if field_arms.is_empty() {
        return Err(ident.span().error("Struct needs fields!").into());
    }

    let final_field_index: usize = field_arms.len() - 1;

    // panic!("fields: {}", meow.len());

    let (impl_generics, type_generics, where_cause) = ast.generics.split_for_impl();

    let outer_wrap_logic = if !struct_attrs.no_wrap {
        quote! {
            match input {
                ::struct_table::ArrowKey::Up if field_index == 0 => {
                    table_state.select(Some(final_field_index));
                    return Ok(self_changed);
                },
                ::struct_table::ArrowKey::Up => {
                    table_state.scroll_up_by(1);
                    return Ok(self_changed);
                },
                ::struct_table::ArrowKey::Down if field_index >= final_field_index => {
                    table_state.select(Some(0));
                    return Ok(self_changed);
                },
                ::struct_table::ArrowKey::Down => {
                    table_state.scroll_down_by(1);
                    return Ok(self_changed);
                },
                _ => (),
            }
        }
    } else {
        // Wrapless behavior
        quote! {
            match input {
                ::struct_table::ArrowKey::Up => {
                    table_state.scroll_up_by(1);
                    return Ok(self_changed);
                },
                ::struct_table::ArrowKey::Down => {
                    table_state.scroll_down_by(1);
                    if let Some(index) = table_state.selected() {
                        if index >= final_field_index {
                            table_state.select(Some(final_field_index));
                        }
                    }
                    return Ok(self_changed);
                },
                _ => (),
            }
        }
    };

    Ok(quote! {
        impl #impl_generics ::struct_table::StructTable for #ident #type_generics #where_cause {
            fn handle_input(&mut self, input: ::struct_table::ArrowKey, table_state: &mut ::ratatui::widgets::TableState) -> ::core::result::Result<bool, ()> {
                let mut self_changed = false;
                let field_index = match table_state.selected() {
                    None => {
                        table_state.select(Some(0));
                        return Ok(self_changed);
                    }
                    Some(index) => index,
                };
                let final_field_index: usize = #final_field_index;
                // Assuming left/right only here
                let next: bool;

                #outer_wrap_logic

                match input {
                    ::struct_table::ArrowKey::Right => next = true,
                    ::struct_table::ArrowKey::Left => next = false,
                    _ => unreachable!(),
                }

                match field_index {
                    #( #field_indices => {
                        #field_arms;
                        self_changed = true;
                       }, )*
                    _ => return Err(()),
                }

                Ok(self_changed)
            }

            fn as_table(&self, table_state: &mut ::ratatui::widgets::TableState) -> ::ratatui::widgets::Table<'_> {
                use ::ratatui::{
                    layout::Constraint,
                    style::{Style, Stylize},
                    text::Text,
                    widgets::{Row, Table},
                };
                table_state.select_first_column();
                let selected_row_style = Style::new().reversed();
                let first_column_style = Style::new().reset();

                let rows: Vec<Row> = vec![
                    #(
                    Row::new([
                        Text::raw(#field_human_names).right_aligned(),
                        Text::raw(#field_string_values).centered(),
                    ])
                    ),*
                ];

                let option_table = Table::new(
                    rows,
                    [Constraint::Percentage(50), Constraint::Percentage(50)],
                )
                .column_highlight_style(first_column_style)
                .row_highlight_style(selected_row_style);

                option_table
            }

            const DOCSTRINGS: &'static [&'static str] = &[ #(#docstrings),* ];
        }
    })
}
/// Checking if a field's type path is *exactly* `bool`.
fn is_bool_field(field: &syn::Field) -> bool {
    if let syn::Type::Path(type_path) = &field.ty {
        type_path.path.is_ident("bool")
    } else {
        false
    }
}
fn extract_field_attrs(ast: &mut DeriveInput) -> deluxe::Result<Vec<StructField>> {
    let mut field_attrs: Vec<StructField> = Vec::new();

    if let syn::Data::Struct(s) = &mut ast.data {
        for field in s.fields.iter_mut() {
            // if let syn::Type::Path(type_path) = &field.ty {
            //     if type_path.path.segments.last().unwrap().ident != "bool" {
            //         panic!("Field type is not bool");
            //     }
            // } else {
            //     panic!("Field type is not a recognized path");
            // }

            // if !is_bool_field(field) {
            //     panic!("Field type is not bool!");
            // }

            let ident = match field.ident.as_ref() {
                Some(id) => id.clone(),
                None => {
                    return Err(syn::Error::new_spanned(
                        field,
                        "tuple structs not supported",
                    ));
                }
            };

            let StructFieldAttributes {
                values,
                display,
                no_wrap: no_inner_wrap,
                rename,
                skip,
            } = deluxe::extract_attributes(field)?;

            if skip {
                continue;
            }

            let doc = get_first_doc_comment(&field.attrs).unwrap_or_default();
            // let doc = "".to_string();

            let is_bool = is_bool_field(field);

            if !is_bool && values.is_none() {}

            // Some of these checks we can only do if the user
            // supplied an array directly.
            // Extra checks are added in const {} contexts to ensure correctness.

            // Verifying validity of values_to_cycle values
            match (is_bool, &values) {
                (false, None) => {
                    return Err(ident
                        .span()
                        .error("expected #[table(values = [])] with array of values")
                        .into());
                }
                (false, Some(ArrayOrConst::Array(values))) if values.elems.is_empty() => {
                    return Err(ident
                        .span()
                        .error("table values array cannot be empty")
                        .into());
                }
                (true, Some(_)) => {
                    return Err(ident
                        .span()
                        .error("bools can't have other cycled values")
                        .into());
                }
                (false, Some(_)) => (),
                (true, _) => (),
            }

            // This is one check that I haven't thought of a way to reproduce
            // in a const {} context, since iterators/new collections aren't allowed
            // in const {}s.
            if let Some(ArrayOrConst::Array(values)) = &values {
                let value_elems = &values.elems;
                let unique_count: usize = value_elems
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                if unique_count != value_elems.len() {
                    return Err(ident
                        .span()
                        .error("duplicates not allowed in cycled values")
                        .into());
                }
            }

            // Verifying validity of display_override values
            match (is_bool, &display, &values) {
                (true, Some(ArrayOrConst::Array(labels)), _) if labels.elems.len() != 2 => {
                    return Err(ident
                        .span()
                        .error("bools require exactly 2 labels: [true, false]")
                        .into());
                }
                (false, Some(ArrayOrConst::Array(labels)), Some(ArrayOrConst::Array(values)))
                    if labels.elems.len() != values.elems.len() =>
                {
                    return Err(ident
                        .span()
                        .error("display overrides and cycled values must have equal number of elements")
                        .into());
                }
                (_, Some(ArrayOrConst::Array(labels)), _) if labels.elems.is_empty() => {
                    return Err(ident
                        .span()
                        .error("display overrides array cannot be empty")
                        .into());
                }
                (_, _, _) => (),
            }

            let processed_field = StructField {
                ident,
                doc,
                values_to_cycle: values,
                display_override: display,
                is_bool,
                no_inner_wrap,
                rename,
            };
            field_attrs.push(processed_field);
        }
    }

    Ok(field_attrs)
}

fn get_first_doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(meta_name_value) = &attr.meta {
                // Uhhhh, this can probably be done better
                if let syn::Meta::NameValue(meta_name_value) = &attr.meta {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(ref lit_str),
                        ..
                    }) = meta_name_value.value
                    {
                        // let comment: String = format!("\"{}\"", lit_str.value().trim());
                        let comment: String = lit_str.value().trim().to_owned();
                        return Some(comment);
                    }
                }

                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(ref lit_str),
                    ..
                }) = meta_name_value.value
                {
                    // let comment: String = format!("\"{}\"", lit_str.value().trim());
                    let comment: String = lit_str.value().trim().to_owned();
                    return Some(comment);
                }
            }
        }
    }
    None
}

enum ArrayOrConst {
    Array(syn::ExprArray),
    Expr(syn::Expr),
}

impl deluxe::ParseMetaItem for ArrayOrConst {
    fn parse_meta_item(
        input: syn::parse::ParseStream,
        _mode: deluxe::ParseMode,
    ) -> deluxe::Result<Self> {
        // If it starts with `[`, assume it's an array
        if input.peek(syn::token::Bracket) {
            let expr_array: syn::ExprArray = input.parse()?;
            Ok(ArrayOrConst::Array(expr_array))
        } else {
            // Otherwise assume it's an ident/expr.
            let ident: syn::Expr = input.parse()?;
            Ok(ArrayOrConst::Expr(ident))
        }
    }
}

impl quote::ToTokens for ArrayOrConst {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            ArrayOrConst::Array(expr_array) => expr_array.to_tokens(tokens),
            ArrayOrConst::Expr(ident) => ident.to_tokens(tokens),
        }
    }
}

struct StructField {
    ident: syn::Ident,
    doc: String,
    values_to_cycle: Option<ArrayOrConst>,
    display_override: Option<ArrayOrConst>,
    is_bool: bool,
    no_inner_wrap: bool,
    rename: Option<String>,
}

#[derive(deluxe::ExtractAttributes)]
#[deluxe(attributes(table))]
struct StructFieldAttributes {
    /// The values this field will cycle through
    #[deluxe(default)]
    values: Option<ArrayOrConst>,
    /// Override for to_string() for each of the cycled values
    #[deluxe(default)]
    display: Option<ArrayOrConst>,
    /// Don't wrap around when at the end/start of the field's cycled values
    #[deluxe(default)]
    no_wrap: bool,
    /// Override the displayed name for the field, otherwise
    /// is the field's name converted to `Title Case`
    #[deluxe(default)]
    rename: Option<String>,
    /// Don't use this field in the StructTable impls.
    #[deluxe(default)]
    skip: bool,
}

#[derive(deluxe::ExtractAttributes)]
#[deluxe(attributes(table))]
struct StructTableAttributes {
    #[deluxe(default)]
    no_wrap: bool,
    // skip_field_case_conversion: bool
}
