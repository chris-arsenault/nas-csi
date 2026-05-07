# hack

Local development and lab scripts.

No production logic should live here. Scripts in this directory may assume the
single target lab environment and should be promoted into Rust code only when
they become part of normal operation.

Scripts:

- `validate-examples.py`: parse and lightly validate example intent files.
