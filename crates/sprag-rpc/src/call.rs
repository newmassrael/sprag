//! THE READ HALF of a published call grammar — turning what a daemon answered into a call.
//!
//! # Why publishing was only half the feature
//!
//! [`crate::grammar`] is the WRITE half: a surface declares how its verbs may be called and answers
//! that on `action_grammar`. Three rounds built it and every consumer of it was a human reader or a
//! test — `sprag show-grammar` prints the answer and stops there. **Nothing in this workspace has
//! ever built a request out of a published grammar**, so the surface's central claim (*"a client
//! that cannot ask what a word may be has to know it out of band, which for an AI client means
//! guessing"*) was still only half paid: a client could ask, and then had to hand-write the call
//! anyway.
//!
//! This module closes the loop. [`PublishedForm::fill`] takes the flags a person or an agent
//! supplied and the form the DAEMON published, and produces the `args` object — or a refusal that
//! names what the daemon would have refused, before a byte goes out. So a mouth built on it has no
//! second list of argument names in it, and a daemon that grew an argument since this binary was
//! compiled offers it anyway.
//!
//! # Why it is owned and not borrowed
//!
//! [`ArgGrammar`] is `&'static` because a surface DECLARES its grammar at compile time. A client
//! READS one at run time, off a socket, from a daemon that may be a different build — which is the
//! whole reason `show-grammar` asks the daemon instead of printing a table. So the read side owns
//! its strings, and the two halves are held together by
//! [`a_published_grammar_reads_back_as_what_was_declared`](self).
//!
//! # The flag spelling, and the one thing it must not do
//!
//! An argument named `max_iterations` is offered as `--max-iterations` AND `--max_iterations`,
//! because a flag has two spellings the day somebody types the other one. Matching normalises `-`
//! to `_`, so neither spelling is the "real" one and no reverse mapping has to stay correct.
//!
//! `--key=value` is accepted beside `--key value` for the reason R350 recorded against
//! `--settings`: a flag with one spelling is a flag whose other spelling nobody tests. Here it also
//! carries weight — it is the only way to pass a value that begins with `-`.

use serde_json::{Map, Number, Value};

use crate::grammar::{ArgGrammar, CallForm, FormKind};

/// ONE ARGUMENT AS A CLIENT READ IT BACK — the owned counterpart of [`ArgGrammar`].
///
/// Every field is the same fact under the same name; what differs is ownership and that the
/// closed vocabulary is a `Vec` rather than a `&'static [&'static str]`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PublishedArg {
    /// The key to send this argument under.
    pub name: String,
    /// Its JSON type, in `$schema`'s vocabulary (`"int"`, `"string"`, `"bool"`, `"array"`,
    /// `"object"`).
    pub ty: String,
    /// Whether a well-formed call may leave it out.
    pub optional: bool,
    /// The closed vocabulary it admits, or [`None`] when the value is the caller's own.
    pub words: Option<Vec<String>>,
    /// The arguments inside it — empty for every scalar argument.
    pub fields: Vec<PublishedArg>,
}

/// ONE WAY A VERB MAY BE CALLED, as a client read it back — the owned counterpart of [`CallForm`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PublishedForm {
    /// Where this form's arguments live.
    pub form: FormKind,
    /// The arguments, in declared order.
    pub args: Vec<PublishedArg>,
}

/// WHY A PUBLISHED GRAMMAR COULD NOT BE READ.
///
/// A daemon that answers something this cannot parse is a daemon speaking a shape this build does
/// not know, and the honest thing for a client is to say which key it choked on rather than to
/// guess a default — a client that guessed would build a call the daemon refuses and report the
/// daemon's refusal as if the argument were wrong.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GrammarError {
    /// The answer, or a piece of it, was not the JSON shape the publication uses.
    NotShaped {
        /// What was being read when it went wrong, in the reader's terms.
        what: String,
        /// The JSON kind that turned up instead.
        found: &'static str,
    },
    /// A required key of the publication was missing.
    MissingKey {
        /// What was being read.
        what: String,
        /// The key that was not there.
        key: &'static str,
    },
    /// The form's shape word is not one this build knows.
    UnknownForm {
        /// The word the daemon sent.
        word: String,
    },
}

impl std::fmt::Display for GrammarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotShaped { what, found } => {
                write!(
                    f,
                    "the daemon's {what} was a {found}, not the shape a published grammar has"
                )
            }
            Self::MissingKey { what, key } => {
                write!(f, "the daemon's {what} carries no {key:?}")
            }
            Self::UnknownForm { word } => write!(
                f,
                "the daemon calls this form {word:?}, which this build does not know — its \
                 grammar is newer than this binary",
            ),
        }
    }
}

impl std::error::Error for GrammarError {}

/// WHY THE FLAGS A CALLER GAVE DO NOT MAKE A CALL.
///
/// Every arm carries what the caller may DO about it, because this is the refusal a person reads
/// at a shell prompt and an agent reads in a tool result — and a refusal that only says "no" sends
/// the reader back to ask the same question. That is [`GrammarError`]'s rule and R343's
/// (*"the error an operator reads is part of the fix"*), applied to the mouth this module builds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FillError {
    /// A flag that names no argument of the form.
    UnknownFlag {
        /// What the caller typed.
        flag: String,
        /// Every argument this form does take, in declared order.
        known: Vec<String>,
    },
    /// An argument the form requires that the caller did not give.
    Missing {
        /// The arguments that were not supplied, in declared order.
        names: Vec<String>,
    },
    /// A value that is not of the argument's published type.
    NotThatType {
        /// The argument.
        name: String,
        /// Its published type.
        ty: String,
        /// What the caller wrote.
        given: String,
    },
    /// A value outside the argument's published vocabulary.
    NotThatWord {
        /// The argument.
        name: String,
        /// What the caller wrote.
        given: String,
        /// Every word the argument admits.
        words: Vec<String>,
    },
    /// A flag given more than once for an argument that is not a list.
    Repeated {
        /// The argument.
        name: String,
    },
    /// No published form matches the words the caller chose.
    NoForm {
        /// The discriminating argument, when every form is told apart by one.
        selector: Option<String>,
        /// The words that would each have selected a form.
        words: Vec<String>,
    },
    /// The flags fit more than one published form, so the call is not determined.
    ///
    /// Not reachable for any verb sprag serves today — every alternation it publishes is told apart
    /// by a one-word vocabulary — and it exists rather than being an `unreachable!` because the
    /// grammar is READ off a socket: a daemon of another build may publish forms this one cannot
    /// tell apart, and answering "ambiguous" is the only honest thing a client can do about it.
    Ambiguous {
        /// How many forms the flags fitted.
        count: usize,
    },
    /// This form takes nothing and the caller passed something.
    TakesNothing {
        /// What the caller typed.
        flag: String,
    },
}

impl std::fmt::Display for FillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag { flag, known } => write!(
                f,
                "--{flag} is not an argument of this call. It takes: {}",
                joined(known),
            ),
            Self::Missing { names } => write!(
                f,
                "this call needs {}, and {} not given",
                joined(names),
                if names.len() == 1 {
                    "it was"
                } else {
                    "they were"
                },
            ),
            Self::NotThatType { name, ty, given } => {
                write!(f, "--{name} takes {}, and {given:?} is not one", an(ty))
            }
            Self::NotThatWord { name, given, words } => write!(
                f,
                "--{name} does not take {given:?}. It takes: {}",
                joined(words),
            ),
            Self::Repeated { name } => {
                write!(f, "--{name} was given more than once, and it is not a list",)
            }
            Self::NoForm { selector, words } => match selector {
                Some(selector) => write!(
                    f,
                    "say --{selector} to choose what to call. It takes: {}",
                    joined(words),
                ),
                None => write!(f, "no published form of this verb matches what was given"),
            },
            Self::Ambiguous { count } => write!(
                f,
                "what was given fits {count} of the forms this daemon publishes, so which one to \
                 call is not determined",
            ),
            Self::TakesNothing { flag } => {
                write!(f, "this call takes no arguments, and --{flag} was given")
            }
        }
    }
}

impl std::error::Error for FillError {}

/// `a, b and c` — how every refusal above lists what there is.
fn joined(names: &[String]) -> String {
    match names {
        [] => "nothing".to_owned(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// `an int` / `a string` — the article a refusal needs in front of a published type name.
fn an(ty: &str) -> String {
    let article = if ty.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {ty}")
}

/// ONE FLAG A CALLER GAVE — its name as typed (without the leading dashes) and its value, which a
/// `bool` may omit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Flag {
    /// The name as typed, in either spelling.
    pub name: String,
    /// The value, or [`None`] for a bare flag.
    pub value: Option<String>,
}

impl Flag {
    /// A flag with a value.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
        }
    }

    /// A bare flag — well-formed only for a `bool` argument, where it means `true`.
    #[must_use]
    pub fn bare(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
        }
    }
}

/// Two spellings of one argument name are one name: `-` and `_` are the same character here.
#[must_use]
pub fn same_name(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x == y || (x == b'-' && y == b'_') || (x == b'_' && y == b'-'))
}

impl PublishedArg {
    /// Read one argument out of what a daemon answered.
    ///
    /// # Errors
    ///
    /// [`GrammarError`] when the value is not an argument's shape, or is missing a key the
    /// publication always carries.
    pub fn read(value: &Value, what: &str) -> Result<Self, GrammarError> {
        let map = object(value, what)?;
        Ok(Self {
            name: string(map, ArgGrammar::NAME_KEY, what)?,
            ty: string(map, ArgGrammar::TYPE_KEY, what)?,
            optional: map
                .get(ArgGrammar::OPTIONAL_KEY)
                .and_then(Value::as_bool)
                .ok_or_else(|| GrammarError::MissingKey {
                    what: what.to_owned(),
                    key: ArgGrammar::OPTIONAL_KEY,
                })?,
            words: match map.get(ArgGrammar::ONE_OF_KEY) {
                None | Some(Value::Null) => None,
                Some(value) => Some(words(value, what)?),
            },
            fields: match map.get(ArgGrammar::FIELDS_KEY) {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => array(value, what)?
                    .iter()
                    .map(|field| Self::read(field, what))
                    .collect::<Result<_, _>>()?,
            },
        })
    }

    /// Whether this argument is the one that CHOOSES a form — an argument the caller must give,
    /// admitting exactly one word.
    ///
    /// That is the shape `PluginGrammar` documents for `run`'s four forms
    /// (*"each form's `plugin` publishes only the word that selects it"*), and reading it off the
    /// publication is what lets a client pick a form without knowing which verb it is calling.
    #[must_use]
    pub fn selects_a_form(&self) -> bool {
        !self.optional && self.words.as_ref().is_some_and(|words| words.len() == 1)
    }

    /// This argument as a usage line spells it: `--name <type>`, its one word when it has one, and
    /// square brackets when a call may omit it.
    #[must_use]
    pub fn usage(&self) -> String {
        // A nested argument is offered one field at a time, so its own name never appears — see
        // `PublishedForm::fill`'s flattening — and it takes NO bracket of its own: each field
        // carries the one that says whether that field may be left out, and a bracket around the
        // group would say a caller must give all of them or none.
        if !self.fields.is_empty() {
            return self
                .fields
                .iter()
                .map(PublishedArg::usage)
                .collect::<Vec<_>>()
                .join(" ");
        }
        let body = match self.words.as_deref() {
            Some([only]) => format!("--{} {only}", self.name),
            Some(words) => format!("--{} <{}>", self.name, words.join("|")),
            None => format!("--{} <{}>", self.name, self.ty),
        };
        if self.optional {
            format!("[{body}]")
        } else {
            body
        }
    }

    /// Coerce one caller-supplied value into this argument's published type.
    fn coerce(&self, given: &str) -> Result<Value, FillError> {
        let mistyped = || FillError::NotThatType {
            name: self.name.clone(),
            ty: self.ty.clone(),
            given: given.to_owned(),
        };
        let value = match self.ty.as_str() {
            "string" => Value::from(given),
            "int" => Value::Number(given.parse::<i64>().map_err(|_| mistyped())?.into()),
            "float" => Value::Number(
                given
                    .parse::<f64>()
                    .ok()
                    .and_then(Number::from_f64)
                    .ok_or_else(mistyped)?,
            ),
            "bool" => Value::from(given.parse::<bool>().map_err(|_| mistyped())?),
            // An `object` with no published fields is the only place a caller still writes JSON by
            // hand, and it is exactly the case the publication cannot describe (an arrangement
            // tree). Naming it here rather than refusing keeps that door open instead of closing a
            // verb this build has no vocabulary for.
            "object" | "array" => serde_json::from_str(given).map_err(|_| mistyped())?,
            // A type this build has no rule for is passed through as the string it was typed as:
            // the daemon that published the type is the one that judges the value, and refusing
            // here would make this binary the ceiling on what its daemon may grow.
            _ => Value::from(given),
        };
        if let Some(words) = &self.words
            && !words.iter().any(|word| word == given)
        {
            return Err(FillError::NotThatWord {
                name: self.name.clone(),
                given: given.to_owned(),
                words: words.clone(),
            });
        }
        Ok(value)
    }
}

impl PublishedForm {
    /// Read one form out of what a daemon answered.
    ///
    /// # Errors
    ///
    /// [`GrammarError`] when the value is not a form's shape, or names a form kind this build does
    /// not know.
    pub fn read(value: &Value, what: &str) -> Result<Self, GrammarError> {
        let map = object(value, what)?;
        let word = string(map, CallForm::FORM_KEY, what)?;
        let form = FormKind::ALL
            .into_iter()
            .find(|kind| kind.wire_str() == word)
            .ok_or(GrammarError::UnknownForm { word })?;
        let args = map
            .get(CallForm::ARGS_KEY)
            .ok_or_else(|| GrammarError::MissingKey {
                what: what.to_owned(),
                key: CallForm::ARGS_KEY,
            })?;
        Ok(Self {
            form,
            args: array(args, what)?
                .iter()
                .map(|arg| PublishedArg::read(arg, what))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Read every form of one verb — the value `action_grammar` holds under an action's name.
    ///
    /// # Errors
    ///
    /// [`GrammarError`], as [`read`](Self::read).
    pub fn read_all(value: &Value, what: &str) -> Result<Vec<Self>, GrammarError> {
        array(value, what)?
            .iter()
            .map(|form| Self::read(form, what))
            .collect()
    }

    /// Every argument a caller may name, with nesting FLATTENED one level.
    ///
    /// A nested argument is offered by its fields rather than by itself, because the fields are
    /// what a caller has a value for: `--max-iterations 5`, never
    /// `--guardrails '{"max_iterations":5}'`. The parent is re-assembled in [`fill`](Self::fill).
    ///
    /// ⚠ The flattening is only unambiguous while no field shares a name with a top-level argument
    /// of the same form, which is a property of the DECLARATIONS and is asserted there
    /// (`a_flattened_nested_argument_collides_with_nothing`), not assumed here.
    fn offered(&self) -> Vec<(&PublishedArg, Option<&PublishedArg>)> {
        let mut offered = Vec::new();
        for arg in &self.args {
            if arg.fields.is_empty() {
                offered.push((arg, None));
            } else {
                offered.extend(arg.fields.iter().map(|field| (field, Some(arg))));
            }
        }
        offered
    }

    /// Whether the words a caller gave choose THIS form.
    ///
    /// A form with no discriminating argument is chosen by anything, which is right: a verb with
    /// one form has nothing to choose between.
    fn selected_by(&self, flags: &[Flag]) -> bool {
        self.args
            .iter()
            .filter(|arg| arg.selects_a_form())
            .all(|arg| {
                let Some(word) = arg.words.as_ref().and_then(|words| words.first()) else {
                    return false;
                };
                flags.iter().any(|flag| {
                    same_name(&flag.name, &arg.name) && flag.value.as_ref() == Some(word)
                })
            })
    }

    /// The `args` value for a call of this form, built from what the caller gave.
    ///
    /// # Errors
    ///
    /// [`FillError`] — an unknown flag, a missing required argument, a value of the wrong type or
    /// outside the argument's vocabulary. Every one of these is a refusal the DAEMON would have
    /// made, made here instead so the caller reads it in the argument's own terms.
    pub fn fill(&self, flags: &[Flag]) -> Result<Value, FillError> {
        match self.form {
            FormKind::Nullary => match flags.first() {
                None => Ok(Value::Null),
                Some(flag) => Err(FillError::TakesNothing {
                    flag: flag.name.clone(),
                }),
            },
            FormKind::Scalar => {
                let arg = self
                    .args
                    .first()
                    .ok_or(FillError::Missing { names: Vec::new() })?;
                let value = flags
                    .iter()
                    .find(|flag| same_name(&flag.name, &arg.name))
                    .and_then(|flag| flag.value.clone())
                    .ok_or_else(|| FillError::Missing {
                        names: vec![arg.name.clone()],
                    })?;
                arg.coerce(&value)
            }
            FormKind::Object => self.fill_object(flags),
        }
    }

    fn fill_object(&self, flags: &[Flag]) -> Result<Value, FillError> {
        let offered = self.offered();
        // UNKNOWN FIRST, so a typo is named as a typo instead of surfacing as the missing argument
        // it was meant to be.
        for flag in flags {
            if !offered
                .iter()
                .any(|(arg, _)| same_name(&flag.name, &arg.name))
            {
                return Err(FillError::UnknownFlag {
                    flag: flag.name.clone(),
                    known: offered.iter().map(|(arg, _)| arg.name.clone()).collect(),
                });
            }
        }

        let mut root = Map::new();
        let mut missing = Vec::new();
        for (arg, parent) in &offered {
            let mut given = flags.iter().filter(|flag| same_name(&flag.name, &arg.name));
            let Some(first) = given.next() else {
                if !arg.optional {
                    missing.push(arg.name.clone());
                }
                continue;
            };
            let value = if arg.ty == "array" {
                // A list argument is a REPEATABLE flag: one element per occurrence, taken
                // verbatim. `--endpoint-a=claude --endpoint-a=-p` is how an argv reaches the wire
                // with a leading dash intact, which is why `--key=value` is not a convenience here.
                let mut items: Vec<Value> = Vec::new();
                for flag in std::iter::once(first).chain(given) {
                    let Some(raw) = &flag.value else {
                        return Err(FillError::NotThatType {
                            name: arg.name.clone(),
                            ty: arg.ty.clone(),
                            given: String::new(),
                        });
                    };
                    items.push(Value::from(raw.as_str()));
                }
                Value::Array(items)
            } else {
                if given.next().is_some() {
                    return Err(FillError::Repeated {
                        name: arg.name.clone(),
                    });
                }
                match &first.value {
                    Some(raw) => arg.coerce(raw)?,
                    // A BARE flag means `true`, and only for a bool: for anything else the value
                    // the caller meant is missing, and saying "not that type" with an empty value
                    // is how they find out they dropped it.
                    None if arg.ty == "bool" => Value::Bool(true),
                    None => {
                        return Err(FillError::NotThatType {
                            name: arg.name.clone(),
                            ty: arg.ty.clone(),
                            given: String::new(),
                        });
                    }
                }
            };
            match parent {
                None => {
                    root.insert(arg.name.clone(), value);
                }
                Some(parent) => match root
                    .entry(parent.name.clone())
                    .or_insert_with(|| Value::Object(Map::new()))
                {
                    Value::Object(inner) => {
                        inner.insert(arg.name.clone(), value);
                    }
                    // Unreachable while the entry above is the only writer of this key, and stated
                    // rather than unwrapped: a nested field sharing a name with a top-level
                    // argument is what would put a non-object here, and that collision is what
                    // `a_flattened_nested_argument_collides_with_nothing` forbids at the
                    // declarations.
                    _ => {
                        return Err(FillError::Repeated {
                            name: parent.name.clone(),
                        });
                    }
                },
            }
        }
        if missing.is_empty() {
            Ok(Value::Object(root))
        } else {
            Err(FillError::Missing { names: missing })
        }
    }

    /// How a person is shown to call this form.
    #[must_use]
    pub fn usage(&self) -> String {
        self.args
            .iter()
            .map(PublishedArg::usage)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Build a call for the verb whose published `forms` these are, choosing the form the caller's own
/// words select.
///
/// # How a form is chosen, and why the rule is read off the publication
///
/// A verb with several forms publishes a discriminating argument on each — one that a call must
/// carry and that admits exactly ONE word ([`PublishedArg::selects_a_form`]). So the choice is:
/// keep the forms whose discriminators the caller matched; if exactly one survives, fill it. That
/// rule is stated nowhere in this function's own vocabulary — it is read out of the answer — which
/// is what lets one mouth serve a verb whose forms this binary has never heard of.
///
/// # Errors
///
/// [`FillError::NoForm`] when nothing was selected (its message names the discriminator and every
/// word that would have chosen a form), [`FillError::Ambiguous`] when several forms fit, and
/// whatever [`PublishedForm::fill`] refuses otherwise.
pub fn build_call(forms: &[PublishedForm], flags: &[Flag]) -> Result<Value, FillError> {
    let selected: Vec<&PublishedForm> = forms
        .iter()
        .filter(|form| form.selected_by(flags))
        .collect();
    match selected.as_slice() {
        [one] => one.fill(flags),
        [] => Err(no_form(forms)),
        many => {
            let filled: Vec<Value> = many
                .iter()
                .filter_map(|form| form.fill(flags).ok())
                .collect();
            match filled.len() {
                1 => Ok(filled.into_iter().next().expect("one filled form")),
                0 => many
                    .first()
                    .expect("at least two forms")
                    .fill(flags)
                    .and(Err(FillError::Ambiguous { count: many.len() })),
                count => Err(FillError::Ambiguous { count }),
            }
        }
    }
}

/// The refusal for "nothing was selected", naming the discriminator and its words across the forms.
fn no_form(forms: &[PublishedForm]) -> FillError {
    let (selector, words) = selector_of(forms);
    FillError::NoForm { selector, words }
}

/// THE ARGUMENT THAT CHOOSES A FORM, and every word that would choose one — read off the
/// publication rather than named by the caller.
///
/// A mouth wants this for two things a refusal does not cover: offering the discriminator
/// POSITIONALLY (`sprag orchestrate agent …` rather than `--plugin agent`), and printing one usage
/// line per form under the word that selects it. Both would otherwise be a second place that knows
/// `run`'s discriminator is spelled `plugin`.
///
/// Returns [`None`] for a verb whose forms have no discriminator — a verb with one form, where
/// there is nothing to choose.
#[must_use]
pub fn selector_of(forms: &[PublishedForm]) -> (Option<String>, Vec<String>) {
    let mut selector: Option<String> = None;
    let mut words = Vec::new();
    for form in forms {
        for arg in form.args.iter().filter(|arg| arg.selects_a_form()) {
            selector.get_or_insert_with(|| arg.name.clone());
            if selector.as_deref() == Some(arg.name.as_str())
                && let Some(word) = arg.words.as_ref().and_then(|words| words.first())
            {
                words.push(word.clone());
            }
        }
    }
    (selector, words)
}

/// The published forms of every verb one surface serves — what `action_grammar` answers, read.
///
/// # Errors
///
/// [`GrammarError`] when the answer is not an object of actions, or one of its verbs does not read.
pub fn read_surface(answer: &Value) -> Result<Vec<(String, Vec<PublishedForm>)>, GrammarError> {
    let map = object(answer, "call grammar")?;
    map.iter()
        .map(|(action, forms)| {
            Ok((
                action.clone(),
                PublishedForm::read_all(forms, &format!("grammar for {action:?}"))?,
            ))
        })
        .collect()
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, GrammarError> {
    value.as_object().ok_or_else(|| GrammarError::NotShaped {
        what: what.to_owned(),
        found: kind_of(value),
    })
}

fn array<'a>(value: &'a Value, what: &str) -> Result<&'a Vec<Value>, GrammarError> {
    value.as_array().ok_or_else(|| GrammarError::NotShaped {
        what: what.to_owned(),
        found: kind_of(value),
    })
}

fn string(map: &Map<String, Value>, key: &'static str, what: &str) -> Result<String, GrammarError> {
    match map.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Err(GrammarError::NotShaped {
            what: format!("{what}'s {key:?}"),
            found: kind_of(other),
        }),
        None => Err(GrammarError::MissingKey {
            what: what.to_owned(),
            key,
        }),
    }
}

fn words(value: &Value, what: &str) -> Result<Vec<String>, GrammarError> {
    array(value, what)?
        .iter()
        .map(|word| match word {
            Value::String(text) => Ok(text.clone()),
            other => Err(GrammarError::NotShaped {
                what: format!("{what}'s vocabulary"),
                found: kind_of(other),
            }),
        })
        .collect()
}

/// What a value IS, for a message about a value that is the wrong thing.
const fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shape a plugin `run` publishes, in miniature: two forms told apart by one word, an
    /// optional argument, a closed vocabulary, a list, and a NESTED value.
    ///
    /// Declared as [`CallForm`]s and rendered through `to_answer`, deliberately: a fixture written
    /// as JSON by hand would agree with this module about a shape the WRITE half might not produce,
    /// which is the seam the round-trip claim below exists for.
    const FIRST: &[ArgGrammar] = &[
        ArgGrammar::one_of("plugin", "string", &["agent"]),
        ArgGrammar::open("pane", "int"),
        ArgGrammar::open("prompt", "string"),
        ArgGrammar::open("eof", "bool").optional(),
        ArgGrammar::nested(
            "guardrails",
            &[
                ArgGrammar::open("max_iterations", "int").optional(),
                ArgGrammar::open("max_bytes", "int").optional(),
            ],
        )
        .optional(),
    ];
    const SECOND: &[ArgGrammar] = &[
        ArgGrammar::one_of("plugin", "string", &["dialogue"]),
        ArgGrammar::open("endpoint_a", "array"),
        ArgGrammar::one_of("format_a", "string", &["text", "claude_json"]).optional(),
    ];

    fn published() -> Vec<PublishedForm> {
        let answer = Value::from(
            [CallForm::object(FIRST), CallForm::object(SECOND)]
                .iter()
                .map(CallForm::to_answer)
                .collect::<Vec<_>>(),
        );
        PublishedForm::read_all(&answer, "the fixture").expect("the fixture reads")
    }

    fn flags(pairs: &[(&str, &str)]) -> Vec<Flag> {
        pairs.iter().map(|(k, v)| Flag::new(*k, *v)).collect()
    }

    /// **WHAT A SURFACE DECLARES IS WHAT A CLIENT READS BACK** — the two halves of the grammar,
    /// held together.
    ///
    /// The write half renders `&'static` declarations; the read half owns its strings off a socket.
    /// Nothing but this compares them, and without it a key added to one side is a key the other
    /// silently ignores — which for `fields` would mean a nested argument that publishes and is
    /// never offered.
    #[test]
    fn a_published_grammar_reads_back_as_what_was_declared() {
        let declared = CallForm::object(FIRST);
        let read = PublishedForm::read(&declared.to_answer(), "the fixture").expect("it reads");
        assert_eq!(read.form, declared.form);
        assert_eq!(read.args.len(), declared.args.len());
        for (read, declared) in read.args.iter().zip(declared.args) {
            assert_eq!(read.name, declared.name);
            assert_eq!(read.ty, declared.ty);
            assert_eq!(read.optional, declared.optional);
            assert_eq!(
                read.words.as_ref().map(Vec::len),
                declared.words.map(<[&str]>::len),
            );
            assert_eq!(
                read.fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>(),
                declared.fields.iter().map(|f| f.name).collect::<Vec<_>>(),
                "a nested argument's fields must survive the round trip, or the mouth built on \
                 them offers nothing",
            );
        }
        // THE CONTROL: the comparison can fail. An argument list that is not this one must not
        // read back as equal, or the loop above is asserting about lengths it happens to share.
        let other = PublishedForm::read(&CallForm::object(SECOND).to_answer(), "the fixture")
            .expect("it reads");
        assert_ne!(other.args[1].name, read.args[1].name);
    }

    /// **A CALL IS BUILT FROM THE FORM THE CALLER'S OWN WORD SELECTED** — the whole point of
    /// reading a grammar rather than hard-coding one.
    #[test]
    fn the_word_a_caller_gave_chooses_the_form_and_fills_it() {
        let call = build_call(
            &published(),
            &flags(&[("plugin", "agent"), ("pane", "2"), ("prompt", "hello")]),
        )
        .expect("the agent form fills");
        assert_eq!(
            call,
            json!({"plugin": "agent", "pane": 2, "prompt": "hello"}),
            "an int is sent as a number and a string as a string, from the published type alone",
        );

        // The OTHER form, off the same table, with a list argument and a bounded vocabulary.
        let dialogue = build_call(
            &published(),
            &[
                Flag::new("plugin", "dialogue"),
                Flag::new("endpoint-a", "claude"),
                Flag::new("endpoint-a", "-p"),
                Flag::new("format_a", "claude_json"),
            ],
        )
        .expect("the dialogue form fills");
        assert_eq!(
            dialogue,
            json!({
                "plugin": "dialogue",
                "endpoint_a": ["claude", "-p"],
                "format_a": "claude_json",
            }),
            "a repeated flag is one list, and both spellings of a name are one name",
        );
    }

    /// **A NESTED ARGUMENT IS OFFERED ONE FIELD AT A TIME AND RE-ASSEMBLED** — D1's answer, driven.
    ///
    /// The flag a caller types is `--max-iterations 3`; what goes on the wire is
    /// `guardrails: {max_iterations: 3}`. Without this the loop's safety knobs are reachable only
    /// by hand-writing JSON, which is the state the door was built to end.
    #[test]
    fn a_nested_field_is_a_flag_of_its_own_and_arrives_inside_its_parent() {
        let call = build_call(
            &published(),
            &flags(&[
                ("plugin", "agent"),
                ("pane", "1"),
                ("prompt", "x"),
                ("max-iterations", "3"),
            ]),
        )
        .expect("the guardrail fills");
        assert_eq!(
            call["guardrails"],
            json!({"max_iterations": 3}),
            "the field the caller named, inside the parent they never named",
        );
        // THE CONTROL: the parent is not invented when no field was given. A `guardrails: {}` would
        // be a caller saying "these are my bounds" and meaning nothing.
        let bare = build_call(
            &published(),
            &flags(&[("plugin", "agent"), ("pane", "1"), ("prompt", "x")]),
        )
        .expect("it fills");
        assert!(
            bare.get("guardrails").is_none(),
            "an untouched nested argument must not be sent at all: {bare}",
        );
    }

    /// **EVERY REFUSAL NAMES WHAT THE CALLER MAY DO INSTEAD** — the refusals a person reads at a
    /// prompt and an agent reads in a tool result.
    #[test]
    fn a_refusal_names_what_there_is() {
        let forms = published();
        let cases: Vec<(Vec<Flag>, &str)> = vec![
            (flags(&[("plugin", "agent"), ("pane", "1")]), "prompt"),
            (
                flags(&[("plugin", "agent"), ("pane", "x"), ("prompt", "p")]),
                "int",
            ),
            (
                flags(&[
                    ("plugin", "agent"),
                    ("pane", "1"),
                    ("prompt", "p"),
                    ("nope", "1"),
                ]),
                "nope",
            ),
            (
                flags(&[
                    ("plugin", "dialogue"),
                    ("endpoint_a", "a"),
                    ("format_a", "yaml"),
                ]),
                "claude_json",
            ),
            (flags(&[("pane", "1")]), "dialogue"),
        ];
        for (given, expected) in cases {
            let error = build_call(&forms, &given).expect_err("this call is refused");
            let sentence = error.to_string();
            assert!(
                sentence.contains(expected),
                "the refusal for {given:?} must name {expected:?}: {sentence}",
            );
        }
    }

    /// A `bool` may be a BARE flag, and nothing else may.
    ///
    /// The half that matters is the second: a caller who drops the value off `--prompt` is told
    /// the value is missing, not handed a `true` the daemon would then refuse for a reason about
    /// the wrong argument.
    #[test]
    fn a_bare_flag_is_true_for_a_bool_and_a_mistake_for_anything_else() {
        let filled = build_call(
            &published(),
            &[
                Flag::new("plugin", "agent"),
                Flag::new("pane", "1"),
                Flag::new("prompt", "x"),
                Flag::bare("eof"),
            ],
        )
        .expect("a bare bool fills");
        assert_eq!(filled["eof"], json!(true));

        let error = build_call(
            &published(),
            &[
                Flag::new("plugin", "agent"),
                Flag::new("pane", "1"),
                Flag::bare("prompt"),
            ],
        )
        .expect_err("a bare string is a mistake");
        assert!(
            matches!(&error, FillError::NotThatType { name, .. } if name == "prompt"),
            "{error:?}",
        );
    }

    /// **THE OTHER TWO SHAPES A FORM CAN BE**, which no verb the mouths reach happens to use.
    ///
    /// A `Scalar` form's one argument IS the whole `args` value, and a `Nullary` form has nothing
    /// to fill — both of which a mouth built only against the plugin host's four object forms would
    /// get wrong the first time it met a pane's `text`. They are driven here rather than left to
    /// the day somebody points this at another surface: `fill` is written for the published
    /// vocabulary, not for the one caller it has today.
    #[test]
    fn a_scalar_form_is_its_own_argument_and_a_nullary_one_takes_nothing() {
        const TEXT: ArgGrammar = ArgGrammar::open("text", "string");
        let scalar = PublishedForm::read(&CallForm::scalar(&TEXT).to_answer(), "a pane's text")
            .expect("it reads");
        assert_eq!(
            scalar.fill(&flags(&[("text", "한")])).expect("it fills"),
            json!("한"),
            "the value is the whole args, with no key around it",
        );
        assert!(
            matches!(scalar.fill(&[]), Err(FillError::Missing { names }) if names == ["text"]),
            "and a scalar form with nothing given says which argument is missing",
        );

        let nullary =
            PublishedForm::read(&CallForm::nullary().to_answer(), "a palette open").expect("reads");
        assert_eq!(nullary.fill(&[]).expect("it fills"), Value::Null);
        assert!(
            matches!(
                nullary.fill(&flags(&[("anything", "1")])),
                Err(FillError::TakesNothing { .. }),
            ),
            "a verb that needs nothing is not silently handed something",
        );
    }

    /// **FORMS THIS BUILD CANNOT TELL APART ARE SAID TO BE AMBIGUOUS, NOT PICKED BETWEEN.**
    ///
    /// Unreachable against any surface in this workspace — every alternation sprag publishes is
    /// told apart by a one-word vocabulary — and reachable against a daemon of another build, which
    /// is the whole reason a client reads its grammar off a socket instead of compiling it in.
    /// Guessing there would send a call the daemon refuses and report the refusal as if the
    /// caller's arguments were wrong.
    #[test]
    fn forms_that_cannot_be_told_apart_are_reported_rather_than_guessed_between() {
        const ONE: &[ArgGrammar] = &[ArgGrammar::open("pane", "int")];
        const TWO: &[ArgGrammar] = &[ArgGrammar::open("pane", "int").optional()];
        let forms: Vec<PublishedForm> = [CallForm::object(ONE), CallForm::object(TWO)]
            .iter()
            .map(|form| PublishedForm::read(&form.to_answer(), "a newer daemon").expect("reads"))
            .collect();
        assert!(
            matches!(
                build_call(&forms, &flags(&[("pane", "1")])),
                Err(FillError::Ambiguous { count: 2 }),
            ),
            "two forms with no discriminator both fit, and which was meant is not this client's to \
             decide",
        );
        // THE CONTROL: a discriminator makes the same two forms decidable, which is what sprag's
        // own tables all have.
        assert!(
            build_call(
                &published(),
                &flags(&[("plugin", "agent"), ("pane", "1"), ("prompt", "x")])
            )
            .is_ok()
        );
    }

    /// **A PUBLICATION THIS BUILD CANNOT READ NAMES WHAT IT CHOKED ON** — the two arms a malformed
    /// answer takes, driven rather than left to the day a daemon sends one.
    ///
    /// The failure this guards against is a client that GUESSES past a shape it does not recognise:
    /// it would build a call the daemon refuses and report the daemon's refusal as if the caller's
    /// argument were wrong. So a grammar that is not a grammar is said, and the message names the
    /// key or the kind — which is the whole difference between a bug report and a shrug.
    #[test]
    fn a_publication_that_is_not_one_says_which_part_it_choked_on() {
        // NOT SHAPED: the answer is a scalar where an object belongs.
        let error = PublishedForm::read(&json!(7), "a form").expect_err("a number is not a form");
        assert!(
            matches!(&error, GrammarError::NotShaped { found, .. } if *found == "number"),
            "{error:?}",
        );
        assert!(error.to_string().contains("number"), "{error}");

        // MISSING KEY: the shape is right and a key the publication always carries is absent.
        let error = PublishedForm::read(&json!({"form": "object"}), "a form")
            .expect_err("a form without its arguments");
        assert!(
            matches!(&error, GrammarError::MissingKey { key, .. } if *key == CallForm::ARGS_KEY),
            "{error:?}",
        );
        assert!(error.to_string().contains(CallForm::ARGS_KEY), "{error}");

        // ...and one level down, where an ARGUMENT is missing the key that says it may be omitted —
        // the arm a reader of `optional` depends on and the one a hand-written probe forgets.
        let error = PublishedForm::read(
            &json!({"form": "object", "args": [{"name": "pane", "type": "int"}]}),
            "a form",
        )
        .expect_err("an argument that does not say whether it is optional");
        assert!(
            matches!(&error, GrammarError::MissingKey { key, .. } if *key == ArgGrammar::OPTIONAL_KEY),
            "{error:?}",
        );

        // THE CONTROL: the same shapes, complete, read without complaint — so the three refusals
        // above are about what is MISSING and not about the reader refusing everything.
        let whole = PublishedForm::read(
            &CallForm::object(FIRST).to_answer(),
            "this build's own publication",
        )
        .expect("a complete publication reads");
        assert_eq!(whole.args.len(), FIRST.len());
    }

    /// A form this build cannot read is SAID, not guessed past.
    #[test]
    fn a_form_shape_this_build_does_not_know_is_named() {
        let error = PublishedForm::read(
            &json!({"form": "delimited", "args": []}),
            "a newer daemon's grammar",
        )
        .expect_err("this build has no such form");
        assert!(
            error.to_string().contains("newer than this binary"),
            "{error}",
        );
        // THE CONTROL: the three shapes this build DOES know all read.
        for kind in FormKind::ALL {
            let form = PublishedForm::read(
                &json!({"form": kind.wire_str(), "args": []}),
                "this build's own",
            )
            .expect("a known form reads");
            assert_eq!(form.form, kind);
        }
    }
}
