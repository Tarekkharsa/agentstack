Output ../docs/firstrun.gif

Set Shell "bash"
Set Width 1200
Set Height 820
Set FontSize 14
Set Framerate 24
Set PlaybackSpeed 1.0

Type "./demo-firstrun.sh"
Enter

# The script is non-interactive and prints the whole first-run flow. Wait for
# its final marker instead of using a fixed duration, so warm builds do not
# create long frozen tails and cold builds get enough time.
Wait+Screen@120s /Done\./
Sleep 2s
