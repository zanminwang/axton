"""Check built HTML links, anchors and local assets at the GitHub Pages subpath."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit

SITE = Path(__file__).resolve().parents[1] / 'site'
BASE = 'https://zanminwang.github.io/axton/'


class Page(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.ids = set()
        self.links = []
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if 'id' in values:
            self.ids.add(values['id'])
        for attribute in ('href', 'src'):
            if values.get(attribute):
                self.links.append(values[attribute])


def check(site=SITE):
    pages = {path: Page(path.read_text(encoding='utf-8')) for path in site.rglob('*.html')}
    if not pages:
        raise SystemExit('No built HTML found; build the site first.')
    errors = []
    count = 0
    origin = urlsplit(BASE)
    for path, page in pages.items():
        page_url = urljoin(BASE, path.relative_to(site).as_posix())
        for link in page.links:
            url = urlsplit(urljoin(page_url, link))
            if url.scheme not in ('http', 'https') or url.netloc != origin.netloc:
                continue
            count += 1
            if not url.path.startswith(origin.path):
                errors.append(f'{path.relative_to(site)}: outside project subpath: {link}')
                continue
            target = site / unquote(url.path[len(origin.path):])
            if target.is_dir():
                target /= 'index.html'
            if not target.is_file():
                errors.append(f'{path.relative_to(site)}: missing target: {link}')
            elif url.fragment and target in pages and unquote(url.fragment) not in pages[target].ids:
                errors.append(f'{path.relative_to(site)}: missing anchor: {link}')
    if errors:
        raise SystemExit('\n'.join(errors))
    print(f'Checked {len(pages)} HTML pages and {count} internal links/assets under {origin.path}.')


if __name__ == '__main__':
    check()
