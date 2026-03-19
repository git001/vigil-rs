<?php
// Aggregated health check for /healthz
//
// If this script executes:
//   ✓ Caddy is alive    — the HTTP request reached Caddy and was forwarded
//   ✓ PHP-FPM is alive  — FPM processed this FastCGI request
//
// Additionally we verify the FPM socket is accepting new connections,
// which catches the case where FPM is overloaded or in shutdown.

header('Content-Type: text/plain');

$sock = @stream_socket_client(
    'unix:///run/php-fpm/php-fpm.sock',
    $errno,
    $errstr,
    timeout: 2,
);

if ($sock === false) {
    http_response_code(503);
    echo "php-fpm: unhealthy ($errstr)\n";
    exit;
}

fclose($sock);

http_response_code(200);
echo "ok\n";
