// Review the real frontend with an in-memory bridge and sample data.
// Nothing in this server connects to the installed app or its configuration.
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '../crates/murmur-app/frontend');
const port = Number(process.env.MURMUR_PREVIEW_PORT || 4173);
const types = { '.html': 'text/html', '.css': 'text/css', '.js': 'text/javascript', '.woff2': 'font/woff2' };
const helpRoot = path.resolve(__dirname, '../crates/murmur-core/src/help/articles');
const articles = fs.existsSync(helpRoot) ? fs.readdirSync(helpRoot).filter(f => f.endsWith('.md')).map(f => {
  const markdown = fs.readFileSync(path.join(helpRoot, f), 'utf8');
  return { title: markdown.match(/^# (.+)/m)?.[1] || f, headings: [...markdown.matchAll(/^## (.+)/gm)].map(m => m[1]), markdown };
}) : [];
http.createServer((req, res) => {
  try {
    const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    if (pathname === '/preview-articles.json') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(articles));
      return;
    }
    const file = pathname === '/preview-bridge.js'
      ? path.join(__dirname, 'ui-preview/bridge.js')
      : path.resolve(root, '.' + (pathname === '/' ? '/index.html' : pathname));
    if (file !== path.join(__dirname, 'ui-preview/bridge.js') && !file.startsWith(root + path.sep)) {
      res.writeHead(403); res.end(); return;
    }
    let body = fs.readFileSync(file);
    if (path.extname(file) === '.html') {
      body = body.toString('utf8').replace('<head>', '<head><script src="/preview-bridge.js"></script>');
    }
    res.writeHead(200, { 'Content-Type': types[path.extname(file)] || 'application/octet-stream', 'Cache-Control': 'no-store' });
    res.end(body);
  } catch {
    res.writeHead(404); res.end('Not found');
  }
}).listen(port, '127.0.0.1', () => console.log(`Murmur UI preview: http://127.0.0.1:${port} (sample data only)`));
