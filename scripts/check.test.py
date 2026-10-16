"""R12: doc-only selection cannot hide Rust/config/unknown or missing impact."""
from pathlib import Path
import runpy

select = runpy.run_path(str(Path(__file__).with_name('check.py')))['selection']
for paths, expected in [([], 'full'), (['README.md'], 'docs'), (['AGENTS.md'], 'full'),
                        (['taskq.md'], 'full'), (['docs/design/answer-card.md'], 'web'),
                        (['src/tray.html', 'tests/tray-action.test.cjs'], 'web'),
                        (['src/core.rs', 'README.md'], 'full'), (['Cargo.lock'], 'full'),
                        (['tests/fixtures/status-snapshot.json'], 'full'),
                        (['.github/workflows/tests.yml'], 'full'), (['unknown'], 'full')]:
    assert select(paths) == expected, (paths, select(paths), expected)
print('PASS change selection; unknown impact broadens')
