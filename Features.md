# Features will be added in this fork

## Disable ":" opening command palette
I can already open the command palette with cmd+shift+p so as a vim user it's quite annoying to see that dialog each time I try to save
Instead I want to see the command in the status bar. Like neovim or doom emacs.

## Disable tabs and tree view
I never want to see the tabs and the tree view

## Opened files menu "Space + ,"
Like doom emacs I want to see the list of opened files in the current session. I should be able to fuzzy search

## Magit like UI for version control (low priority)
Simplified version. 
"Space g g" opens it

These are the actions I 99% of the time perform
- Commit
- Create branch
- Switch to a branch
- Fetch
- Pull
- View diffs, when the cursor is on a staged / unstaged file;
  - When tab is pressed  it toggles the diffs of that file
  - When enter is pressed it opens that file
    - When the cursor on a specific line, it goes to that line in the opened file
  
## Projectile like UI to easily switch between projects
- Add a new project
- Open a project
- Switch between projects
- Prompt user to start a clean session or carry over the opened files state

## Make the line numbers container narrower
Right now there are too much white space around the line numbers, it should be 4px on each side

## Mini buffer

## Scratch buffer
A buffer to write down anything any time, it's not bound to a project

## Key bindings
"Space `" focuses on the previously opened file
"Space p f" opens a fuzzy finder to find a file in the project
"cmd + ." triggers auto complete
"cmd + j" and "cmd + k" navigates down / up in every kind of menu
"Space Tab Tab" opens a menu 
"Space p a" Add a project
"Space p p" Open a project
"Space p n" Go to home screen
"Space p d" Delete a project
"Space x" opens the scratch buffer
"Space k" opens a minibuffer to show the type information and focuses on the mini buffer. Vim keys work in the minibuffer
"Cmd + w then d" closes the currently focused buffer
"Cmd + w then w" focuses on the next window (if there are multiple panes)

## Notes
- The implementations don't have to be exact take initiative when there is an easier (built in) way to do it. Key bindings should not be the default they should be my config.
