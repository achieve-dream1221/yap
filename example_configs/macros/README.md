Place `.toml` files in the `macros` folder containing macros to be used with your serial devices. Folders inside of the `macros` will also be recursively searched!

Macros without an explicitly set category will inherit the file's category name.

By default, the stem of the filename will be used for the category.

(Example: `openshock.toml` -> `openshock`)

You can override the default category by placing `category = "New Name"` at the top of the file (above any macros).

Example file:

```toml

# category = "Examples"

[[macro]]
name = "My Cool Macro" ## name in UI and for assigning macro to keybind
category = "Cool Guy Macros" ## (optional) groups macros in the UI, will use file's category name if not provided.
content = "ping 1.1.1.1" ## content to send
line_ending = "\n\n\n" ## (optional) if not provided, uses the configured Macro line ending in Port Settings

[[macro]]
name = "Cooler Macro"
content = "ping 8.8.8.8"
```
