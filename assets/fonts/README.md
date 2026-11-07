# Bundled fonts

Static TTF files, one directory per family. The "Family name" column is the name table
family (ID 1, or ID 16 typographic family where the file has one) as the font reports it.
Medium/SemiBold interface weights carry a legacy ID 1 such as "Geist Medium", but their
typographic family (ID 16) is the plain family name listed here.

| Slug | Family name | Faces | Version | License | Source |
|---|---|---|---|---|---|
| meslolgs-nf | MesloLGS NF | Regular, Bold, Italic, Bold Italic | powerlevel10k-media master | Apache 2.0 (Meslo LG) + MIT (Nerd Fonts), see NOTICE.txt | https://github.com/romkatv/powerlevel10k-media |
| jetbrains-mono | JetBrains Mono | Regular, Bold, Italic, BoldItalic | (pre-existing) | OFL 1.1 | https://github.com/JetBrains/JetBrainsMono |
| geist-mono | Geist Mono | Regular, Bold, Italic, BoldItalic | 1.9.2 | OFL 1.1 | https://github.com/vercel/geist-font/releases/tag/v1.9.2 |
| ibm-plex-mono | IBM Plex Mono | Regular, Bold, Italic, BoldItalic | google/fonts main | OFL 1.1 | https://github.com/google/fonts/tree/main/ofl/ibmplexmono |
| fira-code | Fira Code | Regular, Bold (no italic upstream) | 6.2 | OFL 1.1 | https://github.com/tonsky/FiraCode/releases/tag/6.2 |
| cascadia-code | Cascadia Code | Regular, Bold, Italic, BoldItalic | 2407.24 | OFL 1.1 | https://github.com/microsoft/cascadia-code/releases/tag/v2407.24 |
| hack | Hack | Regular, Bold, Italic, BoldItalic | 3.003 | MIT + Bitstream Vera (LICENSE.md) | https://github.com/source-foundry/Hack/releases/tag/v3.003 |
| source-code-pro | Source Code Pro | Regular, Bold, It, BoldIt | 2.042R-u / 1.062R-i | OFL 1.1 | https://github.com/adobe-fonts/source-code-pro/releases/tag/2.042R-u%2F1.062R-i%2F1.026R-vf |
| commit-mono | CommitMono | 400-Regular, 700-Regular, 400-Italic, 700-Italic (ttfautohint TTFs) | 1.143 | OFL 1.1 | https://github.com/eigilnikolajsen/commit-mono/releases/tag/v1.143 |
| geist | Geist | Regular, Medium, SemiBold | 1.9.2 | OFL 1.1 | https://github.com/vercel/geist-font/releases/tag/v1.9.2 |
| inter | Inter | Regular, Medium, SemiBold | 4.1 | OFL 1.1 | https://github.com/rsms/inter/releases/tag/v4.1 (extras/ttf) |
| ibm-plex-sans | IBM Plex Sans | Regular, Medium, SemiBold | @ibm/plex-sans 1.1.0 | OFL 1.1 | https://github.com/IBM/plex/releases (google/fonts only ships variable files) |

Notes:
- Commit Mono: the italic files' typographic family (ID 16) is "CommitMonoV143", while ID 1 is "CommitMono" for all four.
- Iosevka (SPEC §5.10) is not bundled: its four faces are ~45 MB, which would break the `.dmg < 20 MB` target (SPEC §12). It works when installed system-wide (`brew install --cask font-iosevka`).
- Hack ships LICENSE.md instead of OFL.txt.
