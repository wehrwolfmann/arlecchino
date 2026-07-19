"""
Engine configuration (point 6).

Engines are described in a TOML/JSON config file instead of being hard-coded.
Resolution order for the config:

  1. explicit --config PATH
  2. ./arlecchino.toml  (cwd)
  3. ~/.arlecchino/engines.toml
  4. the packaged default (engines.default.toml next to this file)

Each engine entry:

  [[engine]]
  name   = "vineflower"
  type   = "names"          # "names" = keeps original class names (mergeable)
                            # "renamed" = renames classes (compare-only, unless
                            #              a normaliser maps it back — see jadx)
  kind   = "jar"            # "jar" -> run with `java -jar`, "tool" -> run argv[0]
  path   = "vineflower.jar" # absolute, or relative to engine root (~/.arlecchino/engines)
  args   = ["{jar}", "{out}"]   # {jar} and {out} are substituted
  sources_subdir = ""       # where the .java land under {out} ("sources" for jadx)
  normalize = "none"        # "none" | "jadx"  (defpackage un-wrap, see normalize.py)

The substitution tokens available in args: {jar} {out} {java}.
"""
from __future__ import annotations

import json
import os
import shlex
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

try:
    import tomllib  # py3.11+
except ModuleNotFoundError:  # pragma: no cover
    tomllib = None  # type: ignore


ENGINES_ROOT = Path.home() / ".arlecchino" / "engines"
DEFAULT_CONFIG = Path(__file__).with_name("engines.default.toml")


@dataclass
class Engine:
    name: str
    type: str = "names"          # names | renamed
    kind: str = "jar"            # jar | tool
    path: str = ""
    args: list[str] = field(default_factory=list)
    sources_subdir: str = ""
    normalize: str = "none"      # none | jadx
    enabled: bool = True

    def resolve_path(self, root: Path) -> Path:
        p = Path(os.path.expanduser(self.path))
        return p if p.is_absolute() else (root / p)

    def argv(self, jar: Path, out: Path, root: Path) -> list[str]:
        java = os.environ.get("ARLECCHINO_JAVA", "java")
        binp = self.resolve_path(root)
        subst = {"jar": str(jar), "out": str(out), "java": java, "path": str(binp)}
        argv: list[str] = []
        if self.kind == "jar":
            argv = [java, "-jar", str(binp)]
        elif self.kind == "tool":
            argv = [str(binp)]
        else:
            raise ValueError(f"unknown engine kind: {self.kind}")
        for a in self.args:
            argv.append(a.format(**subst))
        return argv

    def sources_dir(self, out: Path) -> Path:
        if self.sources_subdir and (out / self.sources_subdir).is_dir():
            return out / self.sources_subdir
        # jadx always emits to <out>/sources even if not declared
        if (out / "sources").is_dir():
            return out / "sources"
        return out


def _load_raw(path: Path) -> dict[str, Any]:
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".json":
        return json.loads(text)
    if tomllib is None:
        raise RuntimeError("tomllib unavailable (need Python 3.11+) for TOML config; "
                           "use a .json config instead")
    return tomllib.loads(text)


def find_config(explicit: str | None = None) -> Path:
    if explicit:
        return Path(explicit).expanduser().resolve()
    for cand in (Path.cwd() / "arlecchino.toml",
                 Path.home() / ".arlecchino" / "engines.toml"):
        if cand.exists():
            return cand
    return DEFAULT_CONFIG


def load_engines(explicit: str | None = None) -> tuple[list[Engine], Path, Path]:
    """Return (engines, config_path, engines_root)."""
    cfg = find_config(explicit)
    raw = _load_raw(cfg)
    root = Path(os.path.expanduser(raw.get("engines_root", str(ENGINES_ROOT))))
    engines = []
    for e in raw.get("engine", []):
        engines.append(Engine(
            name=e["name"], type=e.get("type", "names"), kind=e.get("kind", "jar"),
            path=e.get("path", ""), args=list(e.get("args", [])),
            sources_subdir=e.get("sources_subdir", ""),
            normalize=e.get("normalize", "none"),
            enabled=e.get("enabled", True),
        ))
    return engines, cfg, root
