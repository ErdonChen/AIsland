param(
    [Parameter(Mandatory = $true)][string]$ProfileId,
    [Parameter(Mandatory = $true)][string]$NativeEvent,
    [Parameter(Mandatory = $true)][string]$SpoolDirectory
)

$ErrorActionPreference = 'Stop'
$maxInputChars = 65536
function Get-NativeValue {
    param([object]$Object, [string]$Name)
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}
function Get-OptionalText {
    param([object]$Object, [string]$Name)
    $candidate = Get-NativeValue $Object $Name
    if ($candidate -isnot [string]) { return $null }
    $value = ([string]$candidate).Trim()
    if ($value.Length -eq 0) { return $null }
    $utf8 = [Text.Encoding]::UTF8
    $bytes = $utf8.GetBytes($value)
    $count = [Math]::Min($bytes.Length, 1024)
    while ($count -gt 0 -and $count -lt $bytes.Length -and (($bytes[$count] -band 0xC0) -eq 0x80)) { $count-- }
    return $utf8.GetString($bytes, 0, $count)
}
function Get-AgentReplyPreview {
    param([object]$Payload, [string]$Event)
    if ($Event -in @('Stop', 'SubagentStop')) { return Get-OptionalText $Payload 'last_assistant_message' }
    if ($Event -eq 'afterAgentResponse') { return Get-OptionalText $Payload 'text' }
    if ($Event -eq 'post_llm_call') {
        $extra = Get-NativeValue $Payload 'extra'
        if ($extra -is [pscustomobject]) { return Get-OptionalText $extra 'assistant_response' }
    }
    return $null
}
$reader = [Console]::In
$buffer = New-Object char[] 4096
$builder = New-Object System.Text.StringBuilder
while (($read = $reader.Read($buffer, 0, $buffer.Length)) -gt 0) {
    if (($builder.Length + $read) -gt $maxInputChars) { exit 65 }
    [void]$builder.Append($buffer, 0, $read)
}

try { $payload = $builder.ToString() | ConvertFrom-Json -ErrorAction Stop } catch { exit 66 }
if ($null -eq $payload -or $payload -is [System.Array]) { exit 67 }
$eventName = [string]$payload.hook_event_name
$sessionId = [string](Get-NativeValue $payload 'session_id')
if ([string]::IsNullOrWhiteSpace($sessionId)) { $sessionId = [string](Get-NativeValue $payload 'conversation_id') }
if (-not $eventName.Equals($NativeEvent, [System.StringComparison]::OrdinalIgnoreCase)) { exit 68 }
if ($sessionId.Length -lt 1 -or $sessionId.Length -gt 128 -or $sessionId -notmatch '^[A-Za-z0-9._:@-]+$') { exit 69 }
if ($ProfileId.Length -lt 1 -or $ProfileId.Length -gt 64 -or $ProfileId -notmatch '^[a-z0-9-]+$') { exit 70 }

$sourceEventId = [Guid]::NewGuid().ToString('D')
$generationId = [string](Get-NativeValue $payload 'generation_id')
if ($generationId.Length -ge 1 -and $generationId.Length -le 64 -and $generationId -match '^[A-Za-z0-9._:@-]+$') {
    $sourceEventId = "$NativeEvent`:$generationId"
}
$event = [ordered]@{
    profileId = $ProfileId
    nativeEvent = $NativeEvent
    taskId = $sessionId
    sourceEventId = $sourceEventId
    occurredAt = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
}
$replyPreview = Get-AgentReplyPreview $payload $NativeEvent
if ($null -ne $replyPreview) { $event['latestReplyPreview'] = $replyPreview }
$json = $event | ConvertTo-Json -Compress
[System.IO.Directory]::CreateDirectory($SpoolDirectory) | Out-Null
$name = [Guid]::NewGuid().ToString('N')
$temporary = [System.IO.Path]::Combine($SpoolDirectory, ".$name.tmp")
$destination = [System.IO.Path]::Combine($SpoolDirectory, "$name.json")
$utf8 = New-Object System.Text.UTF8Encoding($false)
$stream = New-Object System.IO.FileStream($temporary, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
try {
    $bytes = $utf8.GetBytes($json)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
} finally {
    $stream.Dispose()
}
[System.IO.File]::Move($temporary, $destination)
