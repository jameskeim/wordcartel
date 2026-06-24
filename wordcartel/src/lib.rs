#![forbid(unsafe_code)]
//! Wordcartel terminal shell (imperative shell over wordcartel-core).
pub mod editor;
pub mod derive;
pub mod nav;
pub mod commands;
pub mod input;
pub mod registry;
pub mod render;
pub mod term;
pub mod prompt;
pub mod file;
pub mod jobs;
pub mod save;
pub mod app;   // App::run + the testable App::step (Task 12)
pub mod swap;
pub mod recovery;
pub mod filter;
pub mod minibuffer;
pub mod export;
pub mod transform;
