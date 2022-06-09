# Gallerica

Gallerica allows you to define collections of images (galleries)
and will randomly select images from them
in a configurable interval.

It can e.g. be used for switching desktop wallpapers,
or to control a smart picture frame.

**Important**: Currently only Unix is supported.

Gallerica first needs to be configured in order to do its job.
The config file is in $XDG_CONFIG_HOME/gallerica/config.toml (or ~/.config/gallerica/config.toml).

On startup,
and then regularly every `update_interval_ms`,
Gallerica will run the command set in the config file,
substituting the placeholder `{image}` with a random file from the current gallery.

```toml
command_line = "feh --bg-fill {image}"
default_gallery = "default"
update_interval_ms = 600000

[[galleries]]
name = "default"
folders = [ "~/wallpapers/normal" ]

[[galleries]]
name = "rainy-day"
folders = [ "~/wallpapers/rainy-day" ]
```

## gallerica-cli

A running gallerica instance can be controlled via a dedicated command line script,
`gallerica-cli`.
It can be used to change the interval in which images are shown,
which gallery is currently used
and to skip images.

For a full list of options run `gallerica-cli --help`.
