use std::collections::BTreeMap;
use std::io::{self, Write};

use clap::ValueEnum;
use color_eyre::eyre::Result;
use serde::Serialize;

use super::{SCHEMA, Step};
use crate::step::Step as TopgradeStep;

#[derive(Serialize)]
pub(super) struct Entry {
    pub(super) id: String,
    label: String,
    pub(super) mode: &'static str,
    pub(super) tools: &'static [&'static str],
}

fn terminal_label(id: &str) -> String {
    id.split('_')
        .map(|word| {
            let mut label = word.to_owned();
            if let Some(first) = label.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            label
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn entries() -> Vec<Entry> {
    let mut rows = BTreeMap::new();
    for step in TopgradeStep::value_variants() {
        if let Some(value) = step.to_possible_value() {
            let id = value.get_name().to_owned();
            rows.insert(
                id.clone(),
                Entry {
                    label: terminal_label(&id),
                    id,
                    mode: "terminal",
                    tools: &[],
                },
            );
        }
    }
    for step in Step::ALL {
        let id = step.id().to_owned();
        rows.insert(
            id.clone(),
            Entry {
                id,
                label: step.label().to_owned(),
                mode: "controlled",
                tools: step.tools(),
            },
        );
    }
    rows.into_values().collect()
}

pub(super) fn write() -> Result<()> {
    #[derive(Serialize)]
    struct Inventory {
        schema_version: u32,
        steps: Vec<Entry>,
    }

    let mut stdout = io::stdout().lock();
    serde_json::to_writer(
        &mut stdout,
        &Inventory {
            schema_version: SCHEMA,
            steps: entries(),
        },
    )?;
    writeln!(stdout)?;
    Ok(())
}
