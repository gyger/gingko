// <gw-math> elements, emitted by Math.elm into card markdown, render their
// LaTeX source with KaTeX. Elm never patches inside a card's markdown, so the
// rendered output is safe; a changed card gets fresh elements.
import katex from 'katex'

customElements.define('gw-math', class extends HTMLElement {
  connectedCallback () {
    // Invalid LaTeX renders as a red error message instead of throwing.
    katex.render(this.dataset.tex, this, {
      displayMode: this.hasAttribute('display'),
      throwOnError: false
    })
  }
})
