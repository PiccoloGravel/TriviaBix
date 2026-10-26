"""A stand-in for art/generate.py --plan in the render tests."""
import sys
# Without the offline backend the real one compares against placeholder art and counts everything.
if "--backend offline" not in " ".join(sys.argv):
    print("8 to make, 0 up to date", file=sys.stderr)
    sys.exit(0)
print("backdrop:a\nbackdrop:b\nclip:c")
print("3 to make, 5 up to date", file=sys.stderr)
