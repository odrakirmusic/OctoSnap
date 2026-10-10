# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_octosnap_global_optspecs
    string join \n h/help V/version
end

function __fish_octosnap_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_octosnap_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_octosnap_using_subcommand
    set -l cmd (__fish_octosnap_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c octosnap -n "__fish_octosnap_needs_command" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c octosnap -n "__fish_octosnap_needs_command" -s V -l version -d 'Print version'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "capture" -d 'Take a screenshot'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "record" -d 'Record a GIF of a rectangle for a fixed time, in this process rather than the app'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "stop-recording" -d 'Stop the recording in progress. Does nothing when nothing is recording'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "open" -d 'Open a .octosnap project for editing'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "settings" -d 'Open the settings window'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "copy-last" -d 'Put the most recent capture back on the clipboard'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "save-last" -d 'Write the most recent capture to the export location'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "pin" -d 'Pin an image to the screen'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "annotate" -d 'Open an image, or a .octosnap project, in the annotation editor'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "open-from-clipboard" -d 'Open the clipboard\'s image in the annotation editor'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "desktop-icons" -d 'Hide, show or toggle the desktop icons'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "add-overlay" -d 'Add an image file to the Quick Access Overlay as a card'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "text" -d 'Recognise the text in an image and copy it'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "history" -d 'Open the capture history'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "restore" -d 'Bring back the most recently closed card'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "ping" -d 'Check that the app answers, starting it if it is not running'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "shell-version" -d 'Report the shell extension\'s version and protocol'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "monitors" -d 'Print the monitor layout as the extension sees it, in logical coordinates'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "status" -d 'Report on the app and the shell extension at once'
complete -c octosnap -n "__fish_octosnap_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -s m -l mode -d 'What to capture' -r -f -a "all-in-one\t'The overlay with its toolbar and the last selection restored'
area\t'Drag out a region'
window\t'Pick a window'
fullscreen\t'The whole monitor under the pointer'
previous-area\t'The last selection again, with no overlay'
self-timer\t'Select a region, then capture after a countdown'"
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -s a -l action -d 'What to do with it, overriding the configured after-capture actions' -r -f -a "copy\t''
save\t''
annotate\t''
upload\t''
pin\t''"
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -s o -l output -d 'Write the capture to this file as a PNG, or to standard output with `-`, once it is taken, and do nothing else with it: no card, no copy, no save' -r -F
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l rect -d 'Skip the overlay and capture exactly this region, as logical x,y,width,height' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l display -d 'Which monitor, by connector name. See `octosnap monitors`' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l timer -d 'Countdown in seconds. Implies --mode self-timer' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l freeze -d 'Freeze the screen while selecting, overriding the setting'
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l no-freeze -d 'Do not freeze the screen, overriding the setting'
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l cursor -d 'Include the pointer, overriding the setting'
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -l no-cursor -d 'Exclude the pointer, overriding the setting'
complete -c octosnap -n "__fish_octosnap_using_subcommand capture" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l rect -d 'Record exactly this region, as logical x,y,width,height. Default: the whole monitor under the pointer' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l display -d 'Which monitor, by connector name. See `octosnap monitors`' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l seconds -d 'How long to record' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l fps -d 'Frames per second; 0 is the screen\'s own rate, and nothing above 50 is honoured, because a GIF cannot play faster' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l max-width -d 'The widest the GIF may be, in pixels; 0 for no cap' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l quality -d 'The encoder\'s quality, from 1 to 100' -r
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -s o -l output -d 'Where to write the GIF. Default: `octosnap-<date>.gif` in the current directory' -r -F
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l target -d 'How pipewiresrc is pointed at the stream (`docs/spikes/11`): a knob for measuring, so it is in no help' -r -f -a "node-id\t'`path=<node id>`, straight from `PipeWireStreamAdded`'
serial\t'`target-object=<object.serial>`, resolved through `pw-cli`'"
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l gif -d 'The output is a GIF, the only format there is; accepted so scripts read right'
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l no-cursor -d 'Leave the pointer out of the recording'
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -l mark-recording -d 'Tell the compositor this is a recording, so it shows its own indicator too'
complete -c octosnap -n "__fish_octosnap_using_subcommand record" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand stop-recording" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand open" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand settings" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand copy-last" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand save-last" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand pin" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand annotate" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand open-from-clipboard" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand desktop-icons" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand add-overlay" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand text" -l linebreaks -d 'Keep the line breaks the page had, overriding the setting'
complete -c octosnap -n "__fish_octosnap_using_subcommand text" -l single-line -d 'Join each paragraph into one line, overriding the setting'
complete -c octosnap -n "__fish_octosnap_using_subcommand text" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand history" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand restore" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand ping" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand shell-version" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand monitors" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand status" -s h -l help -d 'Print help'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "capture" -d 'Take a screenshot'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "record" -d 'Record a GIF of a rectangle for a fixed time, in this process rather than the app'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "stop-recording" -d 'Stop the recording in progress. Does nothing when nothing is recording'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "open" -d 'Open a .octosnap project for editing'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "settings" -d 'Open the settings window'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "copy-last" -d 'Put the most recent capture back on the clipboard'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "save-last" -d 'Write the most recent capture to the export location'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "pin" -d 'Pin an image to the screen'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "annotate" -d 'Open an image, or a .octosnap project, in the annotation editor'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "open-from-clipboard" -d 'Open the clipboard\'s image in the annotation editor'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "desktop-icons" -d 'Hide, show or toggle the desktop icons'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "add-overlay" -d 'Add an image file to the Quick Access Overlay as a card'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "text" -d 'Recognise the text in an image and copy it'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "history" -d 'Open the capture history'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "restore" -d 'Bring back the most recently closed card'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "ping" -d 'Check that the app answers, starting it if it is not running'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "shell-version" -d 'Report the shell extension\'s version and protocol'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "monitors" -d 'Print the monitor layout as the extension sees it, in logical coordinates'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "status" -d 'Report on the app and the shell extension at once'
complete -c octosnap -n "__fish_octosnap_using_subcommand help; and not __fish_seen_subcommand_from capture record stop-recording open settings copy-last save-last pin annotate open-from-clipboard desktop-icons add-overlay text history restore ping shell-version monitors status help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
