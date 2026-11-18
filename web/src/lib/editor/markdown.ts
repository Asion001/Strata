// Markdown preview: markdown-it with raw HTML off, then DOMPurify. Nothing in
// a document can run script: no <script>, no event handlers, no javascript:
// links, no SVG/MathML, no forms or styles.
import MarkdownIt from 'markdown-it';
import DOMPurify from 'dompurify';

const md = new MarkdownIt({ html: false, linkify: true, typographer: false });

let hooked = false;
function hook() {
  if (hooked) return;
  hooked = true;
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'A') {
      node.setAttribute('target', '_blank');
      node.setAttribute('rel', 'noopener noreferrer nofollow');
    }
  });
}

export function renderMarkdown(src: string): string {
  hook();
  return DOMPurify.sanitize(md.render(src), {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ['style', 'form', 'input', 'button', 'textarea', 'select', 'iframe', 'object', 'embed'],
    FORBID_ATTR: ['style'],
    ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#|\/|\.\.?\/|[^:]*$)/i
  });
}
