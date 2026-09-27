" Project-local vimrc for Rust development.
" Source this from your ~/.vimrc if you want project-specific settings:
"   autocmd BufRead,BufNewFile *.rs setlocal tabstop=4 shiftwidth=4 expandtab

set tabstop=4
set shiftwidth=4
set expandtab
set number
set relativenumber
set signcolumn=yes
set updatetime=300

let g:rustfmt_autosave = 1
let g:rustfmt_emit_files = 1
let g:rustfmt_fail_silently = 0

autocmd BufWritePre *.rs :RustFmt
