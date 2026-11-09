# The game plays itself (`tot play --auto`): the tape only starts it and waits.
# Record: `vhs demo/cyberpunk.tape` inside the dev shell (`nix develop`).
Output demo/cyberpunk.mp4

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 800
Set Padding 0
Set TypingSpeed 0
Set Framerate 15

Hide
Type "tot play --auto --profile cyberpunk"
Enter
Show

Sleep 45s
