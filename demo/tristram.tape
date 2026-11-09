# The game plays itself (`tot play --auto`): the tape only starts it and waits.
# Record: `vhs demo/tristram.tape` inside the dev shell (`nix develop`).
Output demo/tristram.mp4

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 800
Set Padding 0
Set TypingSpeed 0
Set Framerate 15

Hide
Type "tot play --auto --profile tristram"
Enter
Show

Sleep 45s
