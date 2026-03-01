#!/usr/bin/env python3
"""Build a single-page HTML5 manual from the markdown chapter files.

Embeds each chapter as a <script type="text/markdown"> block to avoid
JSON/JS escaping issues. The browser reads .textContent from each block
and passes it to marked.js for rendering.
"""
import os
import html as html_mod

MANUAL_DIR = os.path.dirname(os.path.abspath(__file__))
OUTPUT = os.path.join(MANUAL_DIR, "jarvis-manual.html")

CHAPTERS = [
    ("01-architecture.md", "Architecture Overview"),
    ("02-getting-started.md", "Getting Started"),
    ("03-configuration.md", "Configuration Reference"),
    ("04-terminal.md", "Terminal & Shell"),
    ("05-tiling.md", "Tiling & Window Management"),
    ("06-webview-ipc.md", "WebView & IPC Bridge"),
    ("07-input-palette.md", "Input & Command Palette"),
    ("08-plugins.md", "Plugin System"),
    ("09-networking.md", "Networking & Social"),
    ("10-renderer.md", "Renderer & Visual Effects"),
]

# Read all chapters and build the data blocks
md_blocks = []
for fname, title in CHAPTERS:
    path = os.path.join(MANUAL_DIR, fname)
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
    chid = fname.replace(".md", "")
    # Escape </script> inside markdown so it doesn't break the script tag
    safe = content.replace("</script>", "<\\/script>")
    md_blocks.append((chid, title, safe))

# Build the hidden script blocks
script_tags = ""
for chid, title, content in md_blocks:
    script_tags += f'<script type="text/markdown" data-id="{chid}" data-title="{html_mod.escape(title)}">\n{content}\n</script>\n'

page = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Jarvis Manual</title>
<style>
:root {{
  --bg: #1e1e2e;
  --bg-sidebar: #181825;
  --bg-code: #11111b;
  --text: #cdd6f4;
  --text-muted: #6c7086;
  --primary: #cba6f7;
  --secondary: #f5c2e7;
  --border: #313244;
  --link: #89b4fa;
  --success: #a6e3a1;
  --warning: #f9e2af;
  --error: #f38ba8;
  --font-mono: 'Menlo', 'Consolas', 'Courier New', monospace;
  --font-ui: -apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', sans-serif;
}}
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
html {{ font-size: 15px; scroll-behavior: smooth; }}
body {{
  background: var(--bg);
  color: var(--text);
  font-family: var(--font-ui);
  line-height: 1.7;
  display: flex;
  min-height: 100vh;
}}

/* ── Sidebar ── */
#sidebar {{
  position: fixed;
  top: 0; left: 0;
  width: 280px;
  height: 100vh;
  overflow-y: auto;
  background: var(--bg-sidebar);
  border-right: 1px solid var(--border);
  padding: 20px 0;
  z-index: 100;
}}
#sidebar::-webkit-scrollbar {{ width: 4px; }}
#sidebar::-webkit-scrollbar-thumb {{ background: var(--border); border-radius: 2px; }}
#sidebar-header {{
  font-size: 18px;
  font-weight: 700;
  color: var(--primary);
  padding: 0 20px 16px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 12px;
}}
.ch-link {{
  display: block;
  padding: 8px 20px;
  color: var(--text-muted);
  text-decoration: none;
  font-size: 13px;
  transition: all 0.15s;
  border-left: 3px solid transparent;
}}
.ch-link:hover {{
  color: var(--text);
  background: rgba(203,166,247,0.05);
}}
.ch-link.active {{
  color: var(--primary);
  border-left-color: var(--primary);
  background: rgba(203,166,247,0.08);
  font-weight: 600;
}}
.ch-num {{
  color: var(--text-muted);
  font-size: 11px;
  margin-right: 8px;
  opacity: 0.6;
  font-weight: 400;
}}
.sub-links {{
  max-height: 0;
  overflow: hidden;
  transition: max-height 0.3s ease;
}}
.ch-link.active + .sub-links {{
  max-height: 3000px;
}}
.sub-link {{
  display: block;
  padding: 3px 20px 3px 44px;
  color: var(--text-muted);
  text-decoration: none;
  font-size: 12px;
  line-height: 1.5;
}}
.sub-link:hover {{ color: var(--text); }}

/* ── Search ── */
#search-box {{
  width: calc(100% - 40px);
  margin: 0 20px 12px;
  padding: 8px 12px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  font-family: var(--font-ui);
  outline: none;
}}
#search-box:focus {{ border-color: var(--primary); }}
#search-box::placeholder {{ color: var(--text-muted); }}

/* ── Main ── */
#content {{
  margin-left: 280px;
  flex: 1;
  max-width: 920px;
  padding: 40px 48px 120px;
}}
.chapter {{ margin-bottom: 48px; padding-top: 20px; }}
.chapter-divider {{
  border: none;
  border-top: 1px solid var(--border);
  margin: 48px 0 0;
}}

/* ── Typography ── */
h1 {{ font-size: 2rem; color: var(--primary); margin: 0 0 16px; padding-bottom: 12px; border-bottom: 2px solid var(--border); }}
h2 {{ font-size: 1.45rem; color: var(--primary); margin: 40px 0 12px; padding-bottom: 8px; border-bottom: 1px solid var(--border); }}
h3 {{ font-size: 1.15rem; color: var(--secondary); margin: 28px 0 8px; }}
h4 {{ font-size: 1rem; color: var(--text); margin: 20px 0 8px; font-weight: 600; }}
h5 {{ font-size: 0.9rem; color: var(--text-muted); margin: 16px 0 6px; font-weight: 600; }}
p {{ margin: 8px 0 12px; }}
a {{ color: var(--link); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
strong {{ color: #f5f5f5; }}
hr {{ border: none; border-top: 1px solid var(--border); margin: 24px 0; }}
blockquote {{
  border-left: 3px solid var(--primary);
  padding: 8px 16px;
  margin: 12px 0;
  color: var(--text-muted);
  background: rgba(203,166,247,0.04);
  border-radius: 0 6px 6px 0;
}}
ul, ol {{ margin: 8px 0 12px 24px; }}
li {{ margin: 4px 0; }}
li > ul, li > ol {{ margin: 4px 0 4px 20px; }}

/* ── Code ── */
code {{
  font-family: var(--font-mono);
  font-size: 0.88em;
  background: var(--bg-code);
  padding: 2px 6px;
  border-radius: 4px;
  color: var(--secondary);
}}
pre {{
  background: var(--bg-code);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 16px;
  overflow-x: auto;
  margin: 12px 0 16px;
  line-height: 1.5;
}}
pre code {{
  background: none;
  padding: 0;
  color: var(--text);
  font-size: 13px;
}}

/* ── Tables ── */
table {{
  width: 100%;
  border-collapse: collapse;
  margin: 12px 0 16px;
  font-size: 0.93em;
}}
th {{
  background: var(--bg-sidebar);
  color: var(--primary);
  text-align: left;
  padding: 10px 12px;
  border: 1px solid var(--border);
  font-weight: 600;
  white-space: nowrap;
}}
td {{
  padding: 8px 12px;
  border: 1px solid var(--border);
  vertical-align: top;
}}
tr:nth-child(even) {{ background: rgba(30,30,46,0.5); }}

/* ── Mobile ── */
#menu-toggle {{
  display: none;
  position: fixed;
  top: 12px; left: 12px;
  z-index: 200;
  background: var(--primary);
  color: var(--bg);
  border: none;
  padding: 8px 14px;
  border-radius: 6px;
  font-size: 16px;
  cursor: pointer;
}}
@media (max-width: 800px) {{
  #sidebar {{ transform: translateX(-100%); transition: transform 0.2s; }}
  #sidebar.open {{ transform: translateX(0); }}
  #content {{ margin-left: 0; padding: 60px 20px 80px; max-width: 100%; }}
  #menu-toggle {{ display: block; }}
  table {{ font-size: 0.8em; }}
}}

/* ── Utilities ── */
#back-to-top {{
  position: fixed;
  bottom: 24px;
  right: 24px;
  background: var(--primary);
  color: var(--bg);
  border: none;
  width: 40px; height: 40px;
  border-radius: 8px;
  font-size: 18px;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.2s;
  z-index: 50;
}}
#back-to-top.visible {{ opacity: 1; }}
#loading {{
  position: fixed;
  top: 50%; left: 50%;
  transform: translate(-50%, -50%);
  color: var(--primary);
  font-size: 18px;
  z-index: 300;
}}
</style>
</head>
<body>

<button id="menu-toggle" onclick="document.getElementById('sidebar').classList.toggle('open')">&#9776;</button>

<nav id="sidebar">
  <div id="sidebar-header">Jarvis Manual</div>
  <input type="text" id="search-box" placeholder="Search documentation...">
  <div id="nav-links"></div>
</nav>

<main id="content">
  <div id="loading">Loading documentation...</div>
</main>

<button id="back-to-top" onclick="window.scrollTo({{top:0}})">&#8593;</button>

<!-- Chapter markdown embedded as script blocks -->
{script_tags}

<script src="https://cdn.jsdelivr.net/npm/marked@15/marked.min.js"></script>
<script>
(function() {{
  const content = document.getElementById('content');
  const navLinks = document.getElementById('nav-links');
  const loading = document.getElementById('loading');

  // Gather all markdown blocks
  const blocks = document.querySelectorAll('script[type="text/markdown"]');

  blocks.forEach((block, i) => {{
    const id = block.dataset.id;
    const title = block.dataset.title;
    const md = block.textContent;

    // Parse markdown to HTML
    const rendered = marked.parse(md, {{ gfm: true, breaks: false }});

    // Chapter divider
    if (i > 0) {{
      const hr = document.createElement('hr');
      hr.className = 'chapter-divider';
      content.appendChild(hr);
    }}

    // Chapter container
    const div = document.createElement('div');
    div.className = 'chapter';
    div.id = id;
    div.innerHTML = rendered;
    content.appendChild(div);

    // Sidebar nav link
    const a = document.createElement('a');
    a.className = 'ch-link';
    a.href = '#' + id;
    a.innerHTML = '<span class="ch-num">' + String(i + 1).padStart(2, '0') + '</span>' + title;
    navLinks.appendChild(a);

    // Sub-links for h2 headings
    const subDiv = document.createElement('div');
    subDiv.className = 'sub-links';
    div.querySelectorAll('h2').forEach(h2 => {{
      if (!h2.id) {{
        h2.id = id + '--' + h2.textContent.trim().toLowerCase()
          .replace(/[^a-z0-9]+/g, '-').replace(/-+$/, '');
      }}
      const sub = document.createElement('a');
      sub.className = 'sub-link';
      sub.href = '#' + h2.id;
      sub.textContent = h2.textContent;
      subDiv.appendChild(sub);
    }});
    navLinks.appendChild(subDiv);
  }});

  // Remove loading indicator
  if (loading) loading.remove();

  // Active chapter tracking via IntersectionObserver
  const chLinks = document.querySelectorAll('.ch-link');
  const chapters = document.querySelectorAll('.chapter');
  const observer = new IntersectionObserver(entries => {{
    entries.forEach(entry => {{
      if (entry.isIntersecting) {{
        chLinks.forEach(l => l.classList.remove('active'));
        const link = document.querySelector('.ch-link[href="#' + entry.target.id + '"]');
        if (link) link.classList.add('active');
      }}
    }});
  }}, {{ rootMargin: '-10% 0px -80% 0px' }});
  chapters.forEach(ch => observer.observe(ch));

  // Search
  document.getElementById('search-box').addEventListener('input', function() {{
    const q = this.value.toLowerCase();
    document.querySelectorAll('.chapter').forEach(ch => {{
      ch.style.display = !q || ch.textContent.toLowerCase().includes(q) ? '' : 'none';
    }});
    document.querySelectorAll('.ch-link').forEach(link => {{
      const lid = link.getAttribute('href').slice(1);
      const ch = document.getElementById(lid);
      const show = ch && ch.style.display !== 'none';
      link.style.display = show ? '' : 'none';
      const subs = link.nextElementSibling;
      if (subs && subs.classList.contains('sub-links'))
        subs.style.display = show ? '' : 'none';
    }});
  }});

  // Back to top visibility
  window.addEventListener('scroll', () => {{
    document.getElementById('back-to-top').classList.toggle('visible', window.scrollY > 400);
  }});

  // Close mobile menu on nav click
  navLinks.addEventListener('click', () => {{
    document.getElementById('sidebar').classList.remove('open');
  }});

  // Activate first chapter
  if (chLinks.length > 0) chLinks[0].classList.add('active');
}})();
</script>
</body>
</html>"""

with open(OUTPUT, "w", encoding="utf-8") as f:
    f.write(page)

size_kb = os.path.getsize(OUTPUT) / 1024
print(f"Built {OUTPUT}")
print(f"  {len(md_blocks)} chapters, {size_kb:.0f} KB")
