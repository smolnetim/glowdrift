# Glowdrift

Desktop app that drives ambient lighting from a smoothed screen colour. This glossary covers the vocabulary of installing, updating and removing it, and of the colour engine that turns the screen into light.

## Language

### Installing and updating

**Setup file**:
The single downloadable installer. It is both what a person runs to install and what the built-in updater downloads to update.
_Avoid_: Installer package, update payload, update file

**Install folder**:
The folder holding the application's program files. It is chosen once, on the first install, and every later update or reinstall reuses it.
_Avoid_: Install path, install directory, install location, app folder

**App data**:
Everything the app stores for itself outside the install folder: its configuration, logs and any saved credentials. It survives updates and, unless the person opts in, uninstalling.
_Avoid_: Settings, user data, config

**Update**:
An install of a newer version over an existing one, run without questions, in the existing install folder.
_Avoid_: Upgrade

**Per-user install**:
An install that belongs to one Windows account and needs no administrator rights, for the first install and for updates.
_Avoid_: Current-user install, local install

### Colour engine

**Region**:
The part of a monitor that drives the lights, given as a fraction of that monitor.
_Avoid_: Area, zone, capture area

**Frame**:
One downscaled image of the Region, taken at a single moment.
_Avoid_: Screenshot, capture (as a noun)

**Capture rate**:
How many Frames per second the engine takes.
_Avoid_: FPS, frame rate

**Target colour**:
The single colour the lights should show right now: the colour reduced from the latest Frame, with the brightness and saturation settings applied. It changes abruptly whenever the picture does.
_Avoid_: Raw colour, source colour, input colour

**Smoothed colour**:
The colour that eases toward the Target colour over time and is what the Outputs actually receive.
_Avoid_: Output colour, current colour

**Output**:
A lighting device or service that receives the Smoothed colour.
_Avoid_: Sink, device

**Output rate**:
How often the Smoothed colour is advanced and sent to the Outputs, independent of the Capture rate.
_Avoid_: Tick rate, update rate

**Settled**:
The state in which the Smoothed colour has reached the Target colour, so the engine stops advancing it until the Target colour changes again.
_Avoid_: Idle, converged
