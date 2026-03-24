# Code Signing Policy

Windows releases of **Agents Commander** are digitally signed to ensure authenticity and integrity.

## Certificate

- **Issued by**: SignPath Foundation
- **Algorithm**: SHA256
- **Private key storage**: SignPath Hardware Security Module (HSM)

All signing requests require manual approval. The private key never leaves the HSM.

## Team

| Role | Member | Responsibility |
|------|--------|----------------|
| **Author** | [Mariano Blua](https://github.com/mblua) | Source code maintenance and development |
| **Approver** | [Mariano Blua](https://github.com/mblua) | Signing request approval |

## Verification

You can verify the digital signature of any `.exe` or `.msi` file:

**Windows Explorer**: Right-click the file > Properties > Digital Signatures tab

**PowerShell**:
```powershell
Get-AuthenticodeSignature "Agents Commander_x64-setup.exe"
```

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

Optional features that transmit data when explicitly enabled by the user:

- **Telegram Bridge**: Sends terminal output to the Telegram Bot API when the user attaches a bot to a session
- **Voice-to-Text**: Sends audio recordings to the Google Gemini API for transcription when the user activates voice input

See [PRIVACY.md](PRIVACY.md) for full details.

## Attribution

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).
