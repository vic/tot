# ToT

ToT is a survival game that runs in your terminal. You are a lone hero in a dark dungeon. Foes come in waves, bosses
come at times you cannot predict, and you grow strong enough to beat them or you do not. The game draws iso-view
scenes with text glyphs, so it needs no graphics card and no window system. The game is a prototype.

Each adventure is a set of data files: the foes, the bosses, the words, the colors and the art. The same engine plays
all of them. Here are two runs, played by the built-in demo player.

<video src="demo/cyberpunk.mp4" controls muted loop width="100%"></video>

*Cyberpunk: a sanitation unit wakes up in neon sector 7 and the grid wants it gone.*

<video src="demo/tristram.mp4" controls muted loop width="100%"></video>

*Tristram: a second adventure on the same engine.*

If a video does not play in your viewer, open the files in `demo/` directly.

## Play it

You need [Nix](https://nixos.org/download) with flakes on. Run the game from this repository without a checkout:

```sh frame="terminal"
$ nix run github:vic/tot -- play
```

The command after `--` goes to the game. With no command, the game prints its usage and exits.

The game needs a terminal that speaks the kitty keyboard protocol, so it can see held keys. Ghostty (1.4.0 or newer),
kitty and Rio work. The terminal must be at least 120 columns by 40 rows. Use a small font. The game starts on a
screen where you pick an adventure and a class.

To watch the game play itself, run the demo. It takes no keys and runs in any terminal:

```sh frame="terminal"
$ nix run github:vic/tot -- play --auto --profile cyberpunk
```

The adventures that ship with the game are `cyberpunk`, `dnd`, `hellish`, `kawai` and `moriah`. Add `--seed 7` to
play the same run again.

## Static binary

The package `static-tot` is a static Linux build. It does not need Nix libraries at run time:

```sh frame="terminal"
$ nix run github:vic/tot#static-tot -- play
```

Each release also has a `tot-<system>.tar.gz` for `x86_64-linux` and `aarch64-linux`. Unpack it and run the `tot` file
inside. Keep the `data` directory next to it.

## Keys

In play mode, `wasd`, `hjkl` or the arrow keys move the hero. A tap on Shift dodges. `x` hacks a foe. `q` and `e`
turn the camera. `:` opens the command line, and Space opens the leader menu, which lists the next keys after a short
wait. In the pick screen, `h` and `l` select a power, Enter takes it and `s` skips. After a death or a win, `r`
starts a new run and `q` quits. Every key is data in `data/bindings.jsonl`, so you can remap it.

## Record the demo

The dev shell has [vhs](https://github.com/charmbracelet/vhs) and the Rust tools. The tape in `demo/` only starts the
demo player and waits, so the game does all the work:

```sh frame="terminal"
$ nix develop
$ vhs demo/cyberpunk.tape
```

## Develop

Every tuning knob is in `data/`. A data file with a bad value fails at load, and the message names the file, the line
and the path of the bad value. To run the tests:

```sh frame="terminal"
$ nix develop -c cargo test --release
$ ./test.sh
```

`test.sh` also runs the linter and gives each test a time limit of one second. The bot player runs without a terminal,
so you can test balance changes:

```sh frame="terminal"
$ tot bot --seeds 16 --move kiter --pick tag
```

Run `tot --help` for the other commands.
