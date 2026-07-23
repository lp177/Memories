// Minimal, dependency-free Markdown renderer for the file-preview pane.
//
// Safety first: memory files can be authored by *other* users (via shares), so
// their content is untrusted. Every character of input is HTML-escaped up front
// and this renderer only ever emits a fixed, whitelisted set of tags — never
// raw HTML from the source — and link targets are sanitised. The output is
// therefore safe to hand to `v-html`. It covers the common subset the memory
// notes use: headings, bold/italic, inline + fenced code, links, ordered and
// unordered lists, blockquotes, horizontal rules and paragraphs.

const CODE_MARK = String.fromCharCode(0); // sentinel wrapping protected inline-code indices

function escapeHtml(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

// Only benign link targets are allowed; anything else renders as plain text so
// a `javascript:`/`data:` URL can never become a live link.
function safeUrl(url) {
  const u = url.trim();
  if (/^(https?:|mailto:)/i.test(u)) return u; // explicit safe schemes
  if (/^[/#]/.test(u)) return u; // site-relative or in-page anchor
  if (/^[^:]*$/.test(u)) return u; // no scheme at all (relative path)
  return null;
}

// Inline formatting for a single block of already-collected text.
function inline(src) {
  let out = escapeHtml(src);
  // Protect inline code spans from any further inline processing. The sentinel
  // is a null char, which cannot occur in normal note text or match the rules
  // below, so ordinary strings like "step 3 done" are never mistaken for it.
  const codes = [];
  out = out.replace(/`([^`]+)`/g, (_, c) => {
    codes.push(c); // already escaped by escapeHtml above
    return `${CODE_MARK}${codes.length - 1}${CODE_MARK}`;
  });
  // Links: [text](url) — bracket/paren chars survive escapeHtml untouched.
  out = out.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, text, url) => {
    const safe = safeUrl(url.replace(/&amp;/g, "&"));
    if (!safe) return text;
    return `<a href="${escapeHtml(safe)}" target="_blank" rel="noopener noreferrer nofollow">${text}</a>`;
  });
  // Bold before italic so `**x**` is not eaten by the single-star rule.
  out = out.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  out = out.replace(/\*([^*\n]+)\*/g, "<em>$1</em>");
  // Restore protected code spans (content stays escaped, not re-escaped).
  const restore = new RegExp(`${CODE_MARK}(\\d+)${CODE_MARK}`, "g");
  out = out.replace(restore, (_, i) => `<code>${codes[+i]}</code>`);
  return out;
}

export function renderMarkdown(src) {
  const lines = String(src ?? "").replace(/\r\n?/g, "\n").split("\n");
  const html = [];
  let para = [];
  let i = 0;

  const flushPara = () => {
    if (para.length) {
      html.push(`<p>${inline(para.join(" "))}</p>`);
      para = [];
    }
  };

  while (i < lines.length) {
    const line = lines[i];

    // Fenced code block ``` ... ```
    if (/^```/.test(line)) {
      flushPara();
      const buf = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i])) buf.push(lines[i++]);
      i++; // consume the closing fence (if present)
      html.push(`<pre><code>${escapeHtml(buf.join("\n"))}</code></pre>`);
      continue;
    }

    // Blank line ends a paragraph.
    if (/^\s*$/.test(line)) {
      flushPara();
      i++;
      continue;
    }

    // ATX heading
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      flushPara();
      const lvl = h[1].length;
      html.push(`<h${lvl}>${inline(h[2].trim())}</h${lvl}>`);
      i++;
      continue;
    }

    // Horizontal rule (---, ***, ___)
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flushPara();
      html.push("<hr>");
      i++;
      continue;
    }

    // Blockquote (run of > lines, rendered recursively)
    if (/^\s*>/.test(line)) {
      flushPara();
      const buf = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*>\s?/, ""));
        i++;
      }
      html.push(`<blockquote>${renderMarkdown(buf.join("\n"))}</blockquote>`);
      continue;
    }

    // Lists — a run of unordered or ordered items (nesting is flattened).
    const ordered = /^\s*\d+[.)]\s+/.test(line);
    const unordered = /^\s*[-*+]\s+/.test(line);
    if (ordered || unordered) {
      flushPara();
      const re = ordered ? /^\s*\d+[.)]\s+(.*)$/ : /^\s*[-*+]\s+(.*)$/;
      const items = [];
      while (i < lines.length) {
        const m = lines[i].match(re);
        if (!m) break;
        items.push(`<li>${inline(m[1])}</li>`);
        i++;
      }
      const tag = ordered ? "ol" : "ul";
      html.push(`<${tag}>${items.join("")}</${tag}>`);
      continue;
    }

    // Otherwise accumulate into the current paragraph.
    para.push(line.trim());
    i++;
  }

  flushPara();
  return html.join("\n");
}
