"""
Arlecchino — a meta-decompiler / J2ME port toolkit.

Runs several Java decompilers on one .jar, normalises their output to a single
canonical class-name space, compiles each, and assembles a *best-of* source tree
(per-class, optionally per-method) keeping the variant with the fewest javac
errors.  Then it can auto-fix common decompiler artifacts, round-trip check the
result back to bytecode, optionally deobfuscate, and run the original jar in a
bundled J2ME emulator (FreeJ2ME).

Independent clean-room implementation of the idea from
"Java Decompiler Diversity and its Application to Meta-decompilation"
(Harrand, Soto-Valero, Monperrus, Baudry — arXiv:2005.11315).  No original
Arlecchino code is used.

License: Apache-2.0.
"""

__version__ = "1.0.0"
