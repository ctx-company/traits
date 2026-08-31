# Diagram standards for visualize HTML artifacts

Distilled from the "HTML Diagram" skill of plannotator/effective-html
(github.com/plannotator/effective-html, skills/html-diagram/SKILL.md);
attribution retained, guidance adapted for this trait's artifacts.

## Choose the model the question demands
- Topology: components and connections.
- Sequence: ordered messages over time.
- Process: steps, branches, handoffs.
- State: transitions and their conditions.
- Hierarchy: containment or ownership.
- Timeline: change over time.
- Matrix: repeated relationships.
- Quantitative: only when magnitude itself matters.

## Rendering
- HTML+CSS first; SVG/Canvas as scale demands. One self-contained file:
  inline CSS and JS, no external services, no build step.
- Legible labels, clear grouping, unambiguous direction and connectors
  BEFORE any interaction or animation.
- Stable node positions when comparing states or steps side by side.
- Minimize edge crossings; arrowheads must be unambiguous.
- Contain broad canvases in pan/scroll regions rather than shrinking
  labels below legibility.
- Legends only when notation genuinely needs clarification.
- Respect prefers-reduced-motion; meaning must survive without
  animation and without color alone (dual-encode with shape or label).

## Delivery
- Test wide and narrow widths: label collisions, clipping, routing.
- Keyboard-operable if interactive; accessible text alternatives.
- The diagram presents the walkthrough; the sibling markdown remains
  the authoritative text of it.
