This archive's git history is stored in ai-git/ (NOT .git/) so that `unzip -o` over your
own working tree cannot clobber your real .git. To inspect the AI's commits:
    git --git-dir=ai-git log --oneline
    git --git-dir=ai-git diff <base>..HEAD
To adopt it as a normal repo here:  mv ai-git .git
