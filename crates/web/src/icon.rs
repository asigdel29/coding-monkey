/*
   File: crates/web/src/icon.rs

   Purpose
   The monkey pixel icon — drop-in replacement for the octogent
   octopus across the deck UI. Encoded as a 16×16 SVG with `rect`
   primitives and `shape-rendering="crispEdges"` so it stays sharp
   at any size (sidebar 18×18, terminal-tab 14×14, hero 96×96).

   Palette captured from the reference image:
       bg          #0f5132   forest green
       cap-dark    #8b3a0e   fez body (darker brown)
       cap-mid     #c26b30   fez highlight + head outline + ear outer
       face        #f8e2c5   cream face fill
       ear-inner   #f4b6ae   pink ear inset
       eye         #000000
       nose        #8b3a0e

   The function returns a static `&'static str`. We deliberately avoid
   inlining a base64 PNG: SVG scales without aliasing, weighs ~700 B,
   and is easily themed by swapping the palette constants.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial monkey icon — pixel-style SVG
                                 replacing the octogent octopus
*/

/// Pixel-art monkey head as an inline SVG. The viewBox is 16×16 so
/// the icon scales by simply setting the parent element's width.
///
/// Use as `view! { <div inner_html=monkey_svg() /> }` from leptos, or
/// drop the string straight into HTML.
pub const MONKEY_SVG_16: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" shape-rendering="crispEdges" width="100%" height="100%" role="img" aria-label="monkey">
  <rect width="16" height="16" fill="#0f5132"/>
  <!-- cap (fez) -->
  <rect x="7" y="1" width="2" height="1" fill="#8b3a0e"/>
  <rect x="6" y="2" width="4" height="1" fill="#c26b30"/>
  <rect x="5" y="3" width="6" height="1" fill="#8b3a0e"/>
  <!-- top of head outline -->
  <rect x="4" y="4" width="8" height="1" fill="#c26b30"/>
  <!-- ears: outer outline + pink inner -->
  <rect x="2" y="6" width="2" height="3" fill="#c26b30"/>
  <rect x="3" y="7" width="1" height="1" fill="#f4b6ae"/>
  <rect x="12" y="6" width="2" height="3" fill="#c26b30"/>
  <rect x="12" y="7" width="1" height="1" fill="#f4b6ae"/>
  <!-- head outline (sides + bottom) -->
  <rect x="3" y="5" width="10" height="1" fill="#c26b30"/>
  <rect x="3" y="6" width="1" height="6" fill="#c26b30"/>
  <rect x="12" y="6" width="1" height="6" fill="#c26b30"/>
  <rect x="4" y="12" width="8" height="1" fill="#c26b30"/>
  <rect x="6" y="13" width="4" height="1" fill="#c26b30"/>
  <!-- face fill -->
  <rect x="4" y="5" width="8" height="7" fill="#f8e2c5"/>
  <!-- eyes -->
  <rect x="6" y="7" width="1" height="2" fill="#000000"/>
  <rect x="9" y="7" width="1" height="2" fill="#000000"/>
  <!-- nostrils -->
  <rect x="6" y="10" width="1" height="1" fill="#8b3a0e"/>
  <rect x="9" y="10" width="1" height="1" fill="#8b3a0e"/>
  <!-- mouth hint -->
  <rect x="7" y="11" width="2" height="1" fill="#c26b30"/>
</svg>"##;

/// Larger 32×32 variant with subtle highlights — used for the hero
/// tile in the deck header. Same palette, doubled grid.
pub const MONKEY_SVG_32: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" shape-rendering="crispEdges" width="100%" height="100%" role="img" aria-label="monkey">
  <rect width="32" height="32" fill="#0f5132"/>
  <!-- cap -->
  <rect x="14" y="2" width="4" height="2" fill="#8b3a0e"/>
  <rect x="12" y="4" width="8" height="2" fill="#c26b30"/>
  <rect x="10" y="6" width="12" height="2" fill="#8b3a0e"/>
  <!-- head outline -->
  <rect x="8" y="8" width="16" height="2" fill="#c26b30"/>
  <rect x="6" y="10" width="20" height="2" fill="#c26b30"/>
  <rect x="6" y="12" width="2" height="12" fill="#c26b30"/>
  <rect x="24" y="12" width="2" height="12" fill="#c26b30"/>
  <rect x="8" y="24" width="16" height="2" fill="#c26b30"/>
  <rect x="12" y="26" width="8" height="2" fill="#c26b30"/>
  <!-- ears -->
  <rect x="2" y="12" width="4" height="6" fill="#c26b30"/>
  <rect x="4" y="14" width="2" height="2" fill="#f4b6ae"/>
  <rect x="26" y="12" width="4" height="6" fill="#c26b30"/>
  <rect x="26" y="14" width="2" height="2" fill="#f4b6ae"/>
  <!-- face fill -->
  <rect x="8" y="10" width="16" height="14" fill="#f8e2c5"/>
  <!-- eyes -->
  <rect x="12" y="14" width="2" height="4" fill="#000000"/>
  <rect x="18" y="14" width="2" height="4" fill="#000000"/>
  <!-- nostrils -->
  <rect x="12" y="20" width="2" height="2" fill="#8b3a0e"/>
  <rect x="18" y="20" width="2" height="2" fill="#8b3a0e"/>
  <!-- mouth hint -->
  <rect x="14" y="22" width="4" height="1" fill="#c26b30"/>
</svg>"##;
