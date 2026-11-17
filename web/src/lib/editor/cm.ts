// CodeMirror 6 setup. This module (and every language mode) is loaded only
// when an editor opens, so the file browser stays small.
import { EditorState, Compartment, type Extension } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { syntaxHighlighting, defaultHighlightStyle, StreamLanguage, bracketMatching, indentOnInput } from '@codemirror/language';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';

export { EditorView };

/** Load the language mode for a file name (by extension); plain text if none. */
export async function languageFor(name: string): Promise<Extension> {
  const lower = name.toLowerCase();
  const ext = lower.includes('.') ? lower.slice(lower.lastIndexOf('.') + 1) : '';
  if (lower === 'dockerfile' || lower.endsWith('.dockerfile')) {
    const { dockerFile } = await import('@codemirror/legacy-modes/mode/dockerfile');
    return StreamLanguage.define(dockerFile);
  }
  switch (ext) {
    case 'md':
    case 'markdown':
      return (await import('@codemirror/lang-markdown')).markdown();
    case 'js':
    case 'mjs':
    case 'cjs':
    case 'jsx':
      return (await import('@codemirror/lang-javascript')).javascript({ jsx: true });
    case 'ts':
    case 'mts':
    case 'tsx':
      return (await import('@codemirror/lang-javascript')).javascript({ typescript: true, jsx: ext === 'tsx' });
    case 'rs':
      return (await import('@codemirror/lang-rust')).rust();
    case 'py':
      return (await import('@codemirror/lang-python')).python();
    case 'go':
      return (await import('@codemirror/lang-go')).go();
    case 'json':
      return (await import('@codemirror/lang-json')).json();
    case 'yaml':
    case 'yml':
      return (await import('@codemirror/lang-yaml')).yaml();
    case 'toml': {
      const { toml } = await import('@codemirror/legacy-modes/mode/toml');
      return StreamLanguage.define(toml);
    }
    case 'html':
    case 'htm':
    case 'svelte':
    case 'vue':
      return (await import('@codemirror/lang-html')).html();
    case 'css':
    case 'scss':
      return (await import('@codemirror/lang-css')).css();
    case 'sql':
      return (await import('@codemirror/lang-sql')).sql();
    case 'sh':
    case 'bash':
    case 'zsh':
    case 'fish': {
      const { shell } = await import('@codemirror/legacy-modes/mode/shell');
      return StreamLanguage.define(shell);
    }
    default:
      return [];
  }
}

// The editor splits lines on LF only, so a lone CR (or the CR of a CRLF in a
// mixed-ending file) stays in the text instead of being normalised away.
const lfOnly = EditorState.lineSeparator.of('\n');

export interface EditorOptions {
  doc: string;
  /** Normalise pasted line breaks to LF? Asked on every paste, so it
   *  follows the file if a reload changes its line-ending mode. */
  normalizePaste: () => boolean;
  name: string;
  readOnly: boolean;
  onChange: (doc: string) => void;
}

export async function createEditor(parent: HTMLElement, o: EditorOptions) {
  const lang = new Compartment();
  const ro = new Compartment();
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: o.doc,
      extensions: [
        lineNumbers(),
        history(),
        drawSelection(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        bracketMatching(),
        indentOnInput(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        EditorView.lineWrapping,
        lfOnly,
        // Pasted text (CRLF from Windows clipboards) follows the file's
        // convention instead of smuggling stray CRs in.
        EditorView.clipboardInputFilter.of((text) => (o.normalizePaste() ? text.replace(/\r\n?/g, '\n') : text)),
        ro.of(EditorState.readOnly.of(o.readOnly)),
        keymap.of([
          indentWithTab,
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap
        ]),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) o.onChange(u.state.doc.toString());
        }),
        lang.of([])
      ]
    })
  });
  languageFor(o.name).then((ext) => view.dispatch({ effects: lang.reconfigure(ext) }));
  return {
    view,
    getDoc: () => view.state.doc.toString(),
    setDoc: (doc: string) =>
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: doc } }),
    setReadOnly: (readOnly: boolean) => view.dispatch({ effects: ro.reconfigure(EditorState.readOnly.of(readOnly)) }),
    destroy: () => view.destroy()
  };
}

/** Side-by-side diff of my text against theirs (read-only). */
export async function createMergeView(parent: HTMLElement, mine: string, theirs: string, name: string) {
  const { MergeView } = await import('@codemirror/merge');
  const lang = await languageFor(name);
  const common = [lineNumbers(), lfOnly, EditorView.lineWrapping, EditorState.readOnly.of(true), syntaxHighlighting(defaultHighlightStyle, { fallback: true }), lang];
  return new MergeView({
    parent,
    a: { doc: mine, extensions: common },
    b: { doc: theirs, extensions: common }
  });
}
