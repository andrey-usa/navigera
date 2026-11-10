# Drive a browser from a program

Programs can keep one browser on a pipe instead: `navigera serve` reads
one JSON command per stdin line, for example
`{"id":1,"op":"goto","url":"https://example.com"}`. The session socket speaks
the same protocol.
