//! Extra training data for the book model: the Open Library sample set from the old
//! Python sorter and a small seed lexicon (weights as in book_sorter.md).

use crate::classifier::NaiveBayes;

const OPEN_LIBRARY: &str = include_str!("../data/book_training.tsv");
const LABEL_WEIGHT: u32 = 4;
const SEED_WEIGHT: u32 = 2;

const SEEDS: &[(&str, &str)] = &[
    ("Art", "art artist painting drawing visual design aesthetics illustration sculpture"),
    ("Business", "business management marketing finance risk control strategy branding economics entrepreneurship propaganda"),
    ("Computers", "computer computing programming software code algorithm data network database security hacking rust python react systems sql tcpip"),
    ("Engineering", "engineering chemical process plant thermodynamics mechanical control manufacturing systems design"),
    ("Fiction", "novel story fiction fantasy mystery adventure saga chronicle dune"),
    ("History", "history historical memoir almanac biography letters events civilization innovation"),
    ("Math", "math mathematics algebra logic proof theorem statistics number geometry linear"),
    ("Philosophy", "philosophy ethics metaphysics reasoning logic critical thinking epistemology ontology argument"),
];

/// Open Library titles (weight 1), label names (weight 4) and seed words (weight 2).
/// Labels that aren't in the model are ignored.
pub fn add_builtin(nb: &mut NaiveBayes, labels: &[String]) {
    for line in OPEN_LIBRARY.lines() {
        if let Some((label, text)) = line.split_once('\t') {
            nb.add(label, text, 1);
        }
    }
    for label in labels {
        nb.add(label, label, LABEL_WEIGHT);
    }
    for (label, seeds) in SEEDS {
        nb.add(label, seeds, SEED_WEIGHT);
    }
}
