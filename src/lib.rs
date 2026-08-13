use std::io::{Read, BufReader};
use std::fs::File;
use pgrx::prelude::*;
use rayon::prelude::*;

::pgrx::pg_module_magic!();
use bumpalo::Bump;

mod multimerge;
// Motor de ordenação principal (novo núcleo: detecção fractal de tendências
// + merge paralelo híbrido por co-rank). Único ponto de verdade do algoritmo,
// compartilhado com o repositório `adaptive-parallel-multimerge-sort`.
use multimerge::multi_merge_sort;

// ==========================================
// HELPERS DE LEITURA DO PROTOCOLO BINÁRIO (COPY BINARY)
// ==========================================

fn read_i16_be(reader: &mut impl Read) -> std::io::Result<i16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(i16::from_be_bytes(buf))
}

fn read_i32_be(reader: &mut impl Read) -> std::io::Result<i32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_be_bytes(buf))
}

/// Lê e descarta o header do formato COPY BINARY do PostgreSQL,
/// deixando o `reader` posicionado no primeiro registro.
fn skip_binary_copy_header(reader: &mut impl Read) -> std::io::Result<()> {
    let mut magic = [0u8; 11];
    reader.read_exact(&mut magic)?;
    let _flags = read_i32_be(reader)?;
    let ext_len = read_i32_be(reader)?;
    if ext_len > 0 {
        let mut dummy = vec![0u8; ext_len as usize];
        reader.read_exact(&mut dummy)?;
    }
    Ok(())
}

// ==========================================
// FUNÇÕES EXPOSTAS AO POSTGRESQL
// ==========================================

/// Ordena inteiros de 32 bits lidos via COPY BINARY para um arquivo temporário.
/// Compara diretamente com `ORDER BY` nativo do Postgres para a mesma consulta.
#[pg_extern]
fn pg_multimerge_binary_copy(query: &str, temp_file_path: &str) -> Result<TableIterator<'static, (name!(val, i32),)>, spi::Error> {
    let copy_query = format!("COPY ({}) TO '{}' WITH (FORMAT BINARY)", query, temp_file_path);
    Spi::run(&copy_query)?;

    let file = File::open(temp_file_path).expect("Falha ao abrir arquivo binário");
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    skip_binary_copy_header(&mut reader).expect("Falha ao ler header do COPY BINARY");

    let mut data: Vec<i32> = Vec::with_capacity(100_000_000);

    loop {
        let num_fields = read_i16_be(&mut reader).unwrap();
        if num_fields == -1 { break; } // -1 = fim do arquivo

        let field_len = read_i32_be(&mut reader).unwrap();
        if field_len == 4 {
            data.push(read_i32_be(&mut reader).unwrap());
        } else if field_len > 0 {
            let mut dummy = vec![0u8; field_len as usize];
            reader.read_exact(&mut dummy).unwrap();
        }
    }

    std::fs::remove_file(temp_file_path).ok();

    // i32 satisfaz Ord + Copy + Send + Sync: usa o motor novo diretamente.
    multi_merge_sort(&mut data);

    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}

/// Ordena inteiros de 64 bits obtidos via SPI (sem passar por arquivo temporário).
#[pg_extern]
fn pg_multimerge_stream_i64(query: &str) -> Result<TableIterator<'static, (name!(val, i64),)>, spi::Error> {
    let mut data: Vec<i64> = Spi::connect(|client| {
        let table = client.select(query, None, &[])?;
        let mut results = Vec::new();
        for row in table {
            if let Some(val) = row.get::<i64>(1)? {
                results.push(val);
            }
        }
        Ok::<Vec<i64>, spi::Error>(results)
    })?;

    multi_merge_sort(&mut data);

    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}

/// Ordena inteiros de 32 bits obtidos via SPI (sem passar por arquivo temporário).
#[pg_extern]
fn pg_multimerge_stream_i32(query: &str) -> Result<TableIterator<'static, (name!(val, i32),)>, spi::Error> {
    let mut data: Vec<i32> = Spi::connect(|client| {
        let table = client.select(query, None, &[])?;
        let mut results = Vec::new();
        for row in table {
            if let Some(val) = row.get::<i32>(1)? {
                results.push(val);
            }
        }
        Ok::<Vec<i32>, spi::Error>(results)
    })?;

    multi_merge_sort(&mut data);

    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}

/// Ordena texto lido via COPY BINARY usando alocação em arena (Bumpalo) +
/// ordenação por ponteiro (`&str`).
///
/// O novo motor (`multi_merge_sort`) exige `T: Copy` — é o que permite pular
/// a inicialização do buffer de trabalho (`unsafe { buffer.set_len(n) }`) e
/// ganhar desempenho. `String` não é `Copy`, mas `&str` é, então todo o
/// caminho de texto passa a alocar as strings numa arena e ordenar só as
/// referências (16 bytes cada: ponteiro + tamanho), convertendo para
/// `String` somente no final, já ordenado.
///
/// AVISO DE SEGURANÇA: propositalmente NÃO uso `multi_merge_sort` aqui.
/// O truque de performance do motor novo (`buffer.set_len(n)` sem
/// inicializar) é são para tipos primitivos (todo padrão de bits é válido),
/// mas é tecnicamente unsound para tipos-referência como `&str`: por um
/// instante o buffer contém referências não inicializadas, o que viola a
/// garantia do Rust de que referências são sempre válidas — mesmo que na
/// prática ninguém leia essa posição antes dela ser sobrescrita. Por isso
/// mantenho `par_sort_unstable` (do próprio Rayon, auditado para qualquer
/// `Ord`) para os dois caminhos de texto, e reservo `multi_merge_sort`
/// apenas para os tipos primitivos (`i32`/`i64`) onde a suposição é
/// realmente válida. Se quiser usar o motor novo também para `&str`, o
/// ajuste correto seria trocar o `set_len` não inicializado por escrita via
/// `MaybeUninit<T>` no núcleo — vale abrir isso como issue no repositório
/// `adaptive-parallel-multimerge-sort`.
#[pg_extern]
fn pg_multimerge_binary_copy_text(query: &str, temp_file_path: &str) -> Result<TableIterator<'static, (name!(val, String),)>, spi::Error> {
    let copy_query = format!("COPY ({}) TO '{}' WITH (FORMAT BINARY)", query, temp_file_path);
    Spi::run(&copy_query)?;

    let file = File::open(temp_file_path).expect("Falha ao abrir arquivo binário");
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    skip_binary_copy_header(&mut reader).expect("Falha ao ler header do COPY BINARY");

    let arena = Bump::with_capacity(512 * 1024 * 1024);
    let mut refs: Vec<&str> = Vec::with_capacity(10_000_000);
    let mut word_buffer = Vec::with_capacity(512);

    loop {
        let num_fields = read_i16_be(&mut reader).unwrap();
        if num_fields == -1 { break; }

        let field_len = read_i32_be(&mut reader).unwrap();
        if field_len > 0 {
            word_buffer.resize(field_len as usize, 0);
            reader.read_exact(&mut word_buffer).unwrap();

            // MD5/texto ASCII: pulamos a validação UTF-8 por performance,
            // igual ao caminho original.
            let s = unsafe { std::str::from_utf8_unchecked(&word_buffer) };
            let arena_str = arena.alloc_str(s);
            refs.push(arena_str);
        }
    }

    std::fs::remove_file(temp_file_path).ok();

    // Ver AVISO DE SEGURANÇA acima: par_sort_unstable, não multi_merge_sort.
    refs.par_sort_unstable();

    // Copiamos para Strings próprias ANTES da arena morrer, para devolver
    // dados seguros ao Postgres.
    let owned_results: Vec<String> = refs.into_iter().map(|s| s.to_string()).collect();

    Ok(TableIterator::new(owned_results.into_iter().map(|s| (s,))))
}

/// Variante explícita de arena, mantida para comparação isolada de custo de
/// alocação (arena vs. heap padrão) nos benchmarks.
#[pg_extern]
fn pg_multimerge_binary_copy_text_arena(query: &str, temp_file_path: &str) -> Result<TableIterator<'static, (name!(val, String),)>, spi::Error> {
    let copy_query = format!("COPY ({}) TO '{}' WITH (FORMAT BINARY)", query, temp_file_path);
    Spi::run(&copy_query)?;

    let file = File::open(temp_file_path).expect("Falha ao abrir arquivo binário");
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    skip_binary_copy_header(&mut reader).expect("Falha ao ler header do COPY BINARY");

    let arena = Bump::with_capacity(512 * 1024 * 1024);
    let mut refs: Vec<&str> = Vec::with_capacity(50_000_000);
    let mut word_buffer = Vec::with_capacity(512);

    loop {
        let num_fields = read_i16_be(&mut reader).unwrap();
        if num_fields == -1 { break; }

        let field_len = read_i32_be(&mut reader).unwrap();
        if field_len > 0 {
            word_buffer.resize(field_len as usize, 0);
            reader.read_exact(&mut word_buffer).unwrap();

            let s = std::str::from_utf8(&word_buffer).unwrap();
            let arena_str = arena.alloc_str(s);
            refs.push(arena_str);
        }
    }

    std::fs::remove_file(temp_file_path).ok();

    // Ver AVISO DE SEGURANÇA em pg_multimerge_binary_copy_text.
    refs.par_sort_unstable();

    let owned_results: Vec<String> = refs.into_iter().map(|s| s.to_string()).collect();

    Ok(TableIterator::new(owned_results.into_iter().map(|s| (s,))))
}
