//! The indexer: crawl the chosen folders under the rules, read what changed, embed it, store it. And the search:
//! embed the query, ask the store, rank.

pub mod crawler;
pub mod indexer;
pub mod rules;
pub mod search;

pub use crawler::{Candidate, Root, crawl};
pub use indexer::{Event, Indexer};
pub use search::{SearchOptions, code_query, search};
