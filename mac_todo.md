# Mac: o que falta para ditar pelo servidor do Windows

O Mac grava o áudio, manda para o PC, recebe o texto pronto e cola. O PC transcreve com o Parakeet e faz o pós-processamento com o GLM. Este arquivo é o checklist. A referência detalhada para a sessão do Claude Code no Mac é `docs/mac-client.md`, e o servidor está em `src/parakeet_dictate/server.py`.

Os trechos em Swift abaixo são esboços. Não foram compilados (foram escritos no Windows).

## 0. Antes, no PC (uma vez)

- [ ] Reiniciar o app do Windows (Quit no tray e abrir pelo ícone), para ter o item **Server for Mac**.
- [ ] Rodar `.\scripts\allow_mac.ps1` na pasta do projeto. Pede administrador. Abre a porta 8765 só para a rede local.
- [ ] Marcar **Server for Mac** no tray. Se o Windows perguntar sobre o Python no firewall, clicar **Permitir acesso** com Privada e Pública marcadas.
- [ ] Clicar **Copy Mac connection info** e mandar as duas linhas (URL e token) para você mesmo no Mac. Se falhar, a URL é `http://192.168.3.2:8765` e o token está em `server_token.txt` na pasta do projeto.
- [ ] Opcional: reservar o IP `192.168.3.2` para o PC no roteador (reserva de DHCP), para a URL não mudar.

## 1. Preparar o Mac

- [ ] `git clone https://github.com/MiguelMachado-dev/parakeet-dictate.git` (ou `git pull` se já clonou).
- [ ] Ter o Xcode ou pelo menos `xcode-select --install`.
- [ ] Mac e PC na mesma rede (não pode ser rede de convidados nem Wi-Fi com isolamento de clientes).

## 2. Testar a rede antes de escrever código

```sh
URL=http://192.168.3.2:8765
TOKEN=cole-o-token-aqui

curl -sS -m 5 -H "Authorization: Bearer $TOKEN" "$URL/v1/health"
# esperado: {"status": "ok", "postprocess": true}

curl -sS -H "Authorization: Bearer $TOKEN" \
  -F "file=@tests/fixtures/pt.wav;type=audio/wav" -F postprocess=false \
  "$URL/v1/audio/transcriptions"
# esperado: {"text": "...", "raw": "...", "polished": false}, igual a tests/fixtures/pt.txt
```

- [ ] `/v1/health` respondeu 200.
- [ ] A transcrição do `pt.wav` bateu com `pt.txt`.
- [ ] O mesmo `curl` sem `-F postprocess=false` voltou com `"polished": true`.

| Sintoma | Causa provável |
| --- | --- |
| Timeout | Regra de firewall faltando, IP errado, NordVPN escondendo o PC da rede local, ou Mac em outra rede |
| Connection refused | **Server for Mac** desligado no tray, ou o app do Windows fechado |
| 401 | Token errado ou faltando |
| 503 | O PC ainda está carregando o modelo; tentar de novo em alguns segundos |

## 3. O que o Mac envia

### `GET /v1/health`

- Header: `Authorization: Bearer <token>`.
- Resposta 200: `{"status": "ok", "postprocess": true|false}`.
- Quando chamar:
  - [ ] No hotkey-down, sem esperar a resposta, em paralelo com o início da gravação. Isso aquece a conexão do PC com o GLM e economiza cerca de 0,9 s na resposta.
  - [ ] Ao abrir o app, ao mudar as configurações e a cada 60 s parado, para mostrar "conectado" no menu.
  - [ ] Falha no health só muda o estado no menu. Sem som de erro e sem bloquear a gravação.

### `POST /v1/audio/transcriptions`

Formato exato da requisição:

```
POST /v1/audio/transcriptions HTTP/1.1
Host: 192.168.3.2:8765
Authorization: Bearer <token>
Content-Type: multipart/form-data; boundary=parakeet-<uuid>
Content-Length: <tamanho do corpo>

--parakeet-<uuid>
Content-Disposition: form-data; name="file"; filename="dictation.wav"
Content-Type: audio/wav

<bytes do WAV>
--parakeet-<uuid>
Content-Disposition: form-data; name="postprocess"

true
--parakeet-<uuid>--
```

As quebras de linha do corpo são CRLF (`\r\n`).

- [ ] Campo `file` (obrigatório): WAV PCM de 16 bits. Mandar 16 kHz, mono. O servidor aceita qualquer taxa e estéreo, mas 16 kHz mono é o que o modelo usa e o menor upload (32 KB por segundo).
- [ ] Campo `postprocess` (opcional): exatamente `true` ou `false`. Sem o campo, vale a escolha do tray do Windows. Qualquer outro valor dá 400.
- [ ] `Content-Length` obrigatório. Mandar o corpo como `Data` inteiro. Upload em stream (chunked) dá 411.
- [ ] Timeout da requisição de pelo menos 60 s. O PC faz uma transcrição por vez, então um pedido pode esperar um ditado local terminar.
- [ ] Uma conexão nova por requisição. O servidor sempre responde com `Connection: close`.
- [ ] Os campos `model`, `language`, `response_format` e `temperature` são aceitos e ignorados.

### Respostas

| Status | Quando | Corpo | O que o Mac faz |
| --- | --- | --- | --- |
| 200 | Transcreveu | `{"text": "...", "raw": "...", "polished": true}` | Cola `text` |
| 200 | GLM falhou ou passou de 8 s | `{"text": "<raw>", "raw": "<raw>", "polished": false}` | Cola `text` (é o texto cru) |
| 200 | Sem fala, ou áudio com menos de 0,1 s | `{"text": "", "raw": "", "polished": false}` | Não cola nada, sem som de erro |
| 400 | Não é WAV de 16 bits, sem campo `file`, não é multipart, ou `postprocess` inválido | `{"error": {"message": "..."}}` | Som de erro, logar a mensagem |
| 401 | Token errado | idem | Som de erro, menu diz "token errado" |
| 404 | Caminho errado | idem | Som de erro |
| 411 | Sem `Content-Length` | idem | Som de erro |
| 413 | Corpo acima de 115.265.536 bytes (cerca de 10 min de 48 kHz estéreo) | idem | Som de erro |
| 503 | Modelo ainda carregando | idem | Som de erro, menu diz "PC carregando" |

- [ ] Em erro de rede, timeout ou status diferente de 200: som de erro, estado de falha no overlay, log, e **nunca colar nada**.

## 4. Gravar e montar o WAV

- [ ] `AVAudioEngine` com um tap no `inputNode`.
- [ ] Converter com `AVAudioConverter` para `AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)`. Com mudança de taxa, usar o `convert(to:error:withInputFrom:)`.
- [ ] Juntar as amostras em memória e, ao parar, gerar o WAV com o cabeçalho de 44 bytes:

| Offset | Bytes | Valor |
| --- | --- | --- |
| 0 | 4 | `RIFF` |
| 4 | 4 | 36 + tamanho dos dados (UInt32 little-endian) |
| 8 | 4 | `WAVE` |
| 12 | 4 | `fmt ` (com espaço) |
| 16 | 4 | 16 |
| 20 | 2 | 1 (PCM) |
| 22 | 2 | 1 (mono) |
| 24 | 4 | 16000 |
| 28 | 4 | 32000 (bytes por segundo) |
| 32 | 2 | 2 (bytes por amostra) |
| 34 | 2 | 16 (bits) |
| 36 | 4 | `data` |
| 40 | 4 | tamanho dos dados em bytes |
| 44 | ... | amostras Int16 little-endian |

```swift
func wavData(_ samples: [Int16], rate: UInt32 = 16_000) -> Data {
    var d = Data()
    func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
    func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
    let size = UInt32(samples.count * 2)
    d.append(contentsOf: Array("RIFF".utf8)); u32(36 + size); d.append(contentsOf: Array("WAVE".utf8))
    d.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(rate); u32(rate * 2); u16(2); u16(16)
    d.append(contentsOf: Array("data".utf8)); u32(size)
    samples.withUnsafeBytes { d.append(contentsOf: $0) }
    return d
}
```

- [ ] Alimentar o medidor do overlay com o mesmo tap (as amostras float, antes da conversão).

## 5. Enviar e ler a resposta

```swift
struct Reply: Decodable { let text: String; let raw: String; let polished: Bool }
struct ServerError: Decodable { struct Detail: Decodable { let message: String }; let error: Detail }

func transcribe(wav: Data, base: URL, token: String, postprocess: Bool?) async throws -> Reply {
    let boundary = "parakeet-\(UUID().uuidString)"
    var req = URLRequest(url: base.appendingPathComponent("v1/audio/transcriptions"))
    req.httpMethod = "POST"
    req.timeoutInterval = 60
    req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")

    var body = Data()
    body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n".utf8))
    body.append(wav)
    body.append(Data("\r\n".utf8))
    if let postprocess {
        body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"postprocess\"\r\n\r\n\(postprocess)\r\n".utf8))
    }
    body.append(Data("--\(boundary)--\r\n".utf8))

    let (data, response) = try await URLSession.shared.upload(for: req, from: body)
    let status = (response as? HTTPURLResponse)?.statusCode ?? 0
    guard status == 200 else {
        let message = (try? JSONDecoder().decode(ServerError.self, from: data))?.error.message ?? "HTTP \(status)"
        throw NSError(domain: "Parakeet", code: status, userInfo: [NSLocalizedDescriptionKey: message])
    }
    return try JSONDecoder().decode(Reply.self, from: data)
}
```

- [ ] `upload(for:from:)` já põe o `Content-Length`. Não usar `httpBodyStream`.
- [ ] Guardar a `Task` da requisição para cancelar no Esc. Cancelar no Mac não para o PC: ele termina aquela transcrição, e o próximo pedido pode esperar alguns segundos.

## 6. App, permissões e configurações

- [ ] App de barra de menu (`NSStatusItem`), sem ícone no Dock (`LSUIElement = true` no `Info.plist`, ou `.accessory`).
- [ ] Menu: estado da conexão, **Settings…**, **Quit**.
- [ ] Empacotar como `.app` com `Info.plist`, porque um executável SwiftPM puro não tem bundle e o macOS não guarda as permissões.
- [ ] Assinar com um certificado fixo (o "Apple Development" grátis do Xcode serve). Com assinatura ad-hoc (`codesign -s -`), o macOS costuma esquecer a permissão de Acessibilidade a cada build.
- [ ] `NSMicrophoneUsageDescription` no `Info.plist` (permissão de Microfone).
- [ ] Permissão de Acessibilidade, para o atalho global e para colar com Cmd+V.
- [ ] Rede local. Não verificado:
  - Se a primeira requisição falhar com erro de ATS (-1022), adicionar `NSAppTransportSecurity` com `NSAllowsLocalNetworking = true`.
  - No macOS 15 ou mais novo, o sistema pode pedir permissão de Rede Local; nesse caso, adicionar `NSLocalNetworkUsageDescription`.
  - Se a permissão foi negada, reativar em Ajustes do Sistema, em Privacidade e Segurança, Rede Local.
  - O `curl` não passa por essas regras, então só o app de verdade testa isso.
- [ ] Configurações:
  - URL e token, guardados no Keychain. O campo aceita colar as duas linhas do **Copy Mac connection info** de uma vez.
  - Hotkey.
  - Adicionar espaço no fim (ligado por padrão).
  - Pós-processamento: seguir o PC, ligado ou desligado, enviado como o campo `postprocess`.

## 7. Atalho e máquina de estados

- [ ] Portar `src/parakeet_dictate/reducer.py` para Swift: enum de estados, enum de eventos e uma função pura `step(state, event) -> (state, [effect])`. Portar os testes de `tests/test_reducer.py`.
  - Trocar o efeito de transcrever local pela requisição HTTP.
  - Tirar o efeito `Polish` e o estado `Polishing`, porque o servidor já devolve o texto polido.
- [ ] `CGEvent.tapCreate` ativo (`.defaultTap`, não `.listenOnly`), em key-down, key-up e flags-changed.
  - Se o sistema desligar o tap (`.tapDisabledByTimeout`), religar com `CGEvent.tapEnable`.
- [ ] Hotkey padrão: Command direito (keycode 54). MacBook não tem Control direito. Teclas modificadoras chegam como flags-changed.
- [ ] Segurar e soltar depois de 300 ms: para e transcreve.
- [ ] Toque rápido (soltar antes de 300 ms): modo mãos livres até apertar de novo.
- [ ] Esc (keycode 53):
  - Durante a gravação, descarta o áudio com o som de cancelar.
  - Durante a requisição, cancela e não cola nada.
  - Só engolir o Esc com o hotkey apertado ou com requisição em andamento.
- [ ] Outra tecla com o hotkey apertado (um atalho tipo Cmd+C): descarta em silêncio, sem som e sem requisição.
- [ ] Hotkey durante a requisição não começa nova gravação. Se a resposta chegar com o hotkey ainda apertado, esperar soltar antes de colar.

## 8. Colar

- [ ] Salvar todos os tipos de todos os itens do `NSPasteboard.general`, não só texto.
- [ ] Escrever `text + " "`, com o tipo `org.nspasteboard.TransientType`, para gerenciadores de clipboard ignorarem.
- [ ] Postar Cmd+V via `CGEvent` (tecla V é o keycode 9, com `.maskCommand`). Marcar o evento em `eventSourceUserData` e ignorar eventos marcados no tap.
- [ ] Esperar 200 ms e restaurar o clipboard salvo, mas só se o `changeCount` não mudou desde a nossa escrita.

## 9. Sons

- [ ] Quatro tons a 12% de volume, cada nota com 60 ms, 8 ms de fade e 20 ms entre notas (`app.py`, classe `Cues`):

| Som | Notas |
| --- | --- |
| Início | 600 Hz, depois 900 Hz |
| Fim | 900 Hz, depois 600 Hz |
| Cancelar | 450 Hz |
| Erro | 300 Hz, 300 Hz |

## 10. Overlay

- [ ] Portar `meter.py`, `overlay_frame.py` e `overlay_scene.py` (puros, sem relógio) e os estilos `styles/fluid.py` e `styles/bars.py` para Core Graphics ou Metal, com as mesmas formas, cores e tempos.
- [ ] Canvas de 400 × 120 pontos, 60 fps, só anima enquanto visível.
- [ ] Aparece 250 ms depois de começar a gravar. Fade in de 150 ms, fade out de 200 ms. Atalho (outra tecla) some sem fade. Done, cancelado e falha duram 0,45 s.
- [ ] Durante a requisição, usar o visual de "polishing" o tempo todo, porque o Mac não sabe quando a transcrição termina e o GLM começa.
- [ ] Posição: centro embaixo da tela da janela em foco, 48 pontos acima do fim do `NSScreen.visibleFrame`.
- [ ] `NSPanel` configurado para nunca pegar foco nem cliques e aparecer em todos os Spaces e sobre apps em tela cheia:
  - sem borda, com `.nonactivatingPanel`;
  - `ignoresMouseEvents = true`, `isOpaque = false`, fundo transparente, nível `.statusBar`;
  - `collectionBehavior` `[.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]`;
  - mostrar com `orderFrontRegardless()`.
- [ ] Escolher o estilo no menu: fluid, bars ou off.

## 11. Testar de ponta a ponta

- [ ] Segurar o hotkey, falar, soltar: o texto aparece num editor seguido de espaço, e o clipboard volta a ter o que tinha antes.
- [ ] Toque rápido: mãos livres até apertar de novo.
- [ ] Esc durante a gravação: som de cancelar, nada colado.
- [ ] Esc durante a requisição: nada colado.
- [ ] Hotkey + C: nada gravado, nada enviado.
- [ ] Desligar o **Server for Mac** no PC: som de erro, nada colado, menu mostra desconectado.
- [ ] Ditar no PC e no Mac ao mesmo tempo: os dois funcionam (um espera o outro).
- [ ] Ditado longo (1 min ou mais) com GLM ligado.

## Depois (opcional)

- [ ] Abrir com o login: `SMAppService.mainApp.register()`.
- [ ] Trocar o token: no PC, apagar `server_token.txt` e desligar e ligar o **Server for Mac**. Depois atualizar o token no Mac.
