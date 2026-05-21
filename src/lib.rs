use std::io::{Read, BufReader};
use std::fs::File;
use pgrx::prelude::*;
use rayon::prelude::*;

::pgrx::pg_module_magic!();
use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;

#[pg_extern]
fn pg_multimerge_binary_copy_text_arena(query: &str, temp_file_path: &str) -> Result<TableIterator<'static, (name!(val, String),)>, spi::Error> {
    
    let copy_query = format!("COPY ({}) TO '{}' WITH (FORMAT BINARY)", query, temp_file_path);
    Spi::run(&copy_query)?;

    let file = File::open(temp_file_path).expect("Falha ao abrir arquivo binário");
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);

    // [Lógica de leitura do header permanece igual...]
    let mut magic = [0u8; 11]; reader.read_exact(&mut magic).unwrap();
    let _flags = read_i32_be(&mut reader).unwrap();
    let ext_len = read_i32_be(&mut reader).unwrap();
    if ext_len > 0 { let mut dummy = vec![0u8; ext_len as usize]; reader.read_exact(&mut dummy).unwrap(); }

    // Inicializar a Arena
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

    // 1. Ordenação rápida de ponteiros
    refs.par_sort_unstable();

    // 2. CONVERSÃO DE SEGURANÇA: 
    // Copiamos os dados da arena para Strings proprietárias ANTES de fechar a função.
    // Assim, a 'arena' pode morrer em paz e o PostgreSQL recebe dados seguros.
    let owned_results: Vec<String> = refs.into_iter()
        .map(|s| s.to_string())
        .collect();

    // 3. Agora o iterador é sobre dados OWNED (String), não referências.
    Ok(TableIterator::new(owned_results.into_iter().map(|s| (s,))))
}

// Usamos `impl Read` para que as funções aceitem nosso Buffer de alta velocidade
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

#[pg_extern]
fn pg_multimerge_binary_copy(query: &str, temp_file_path: &str) -> Result<TableIterator<'static, (name!(val, i32),)>, spi::Error> {
    
    // 1. O Postgres despeja os dados no arquivo instantaneamente
    let copy_query = format!("COPY ({}) TO '{}' WITH (FORMAT BINARY)", query, temp_file_path);
    Spi::run(&copy_query)?;

    // 2. O Segredo da Performance: Buffer de 8MB na RAM
    let file = File::open(temp_file_path).expect("Falha ao abrir arquivo binário");
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);

    let mut magic = [0u8; 11];
    reader.read_exact(&mut magic).unwrap();
    let _flags = read_i32_be(&mut reader).unwrap();
    let ext_len = read_i32_be(&mut reader).unwrap();
    if ext_len > 0 {
        let mut dummy = vec![0u8; ext_len as usize];
        reader.read_exact(&mut dummy).unwrap();
    }

    let mut data = Vec::with_capacity(100_000_000);
    
    loop {
        let num_fields = read_i16_be(&mut reader).unwrap();
        if num_fields == -1 { break; } // -1 = Fim do Arquivo

        let field_len = read_i32_be(&mut reader).unwrap();
        if field_len == 4 { 
            data.push(read_i32_be(&mut reader).unwrap());
        } else if field_len > 0 {
            let mut dummy = vec![0u8; field_len as usize];
            reader.read_exact(&mut dummy).unwrap();
        }
    }

    // 3. Deletamos o arquivo temporário silenciosamente
    std::fs::remove_file(temp_file_path).ok();

    // 4. O seu motor faz a ordenação
    ordenar_multi_merge_generic(&mut data);

    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}


// Função atualizada para reciclar um buffer
fn read_string_be_opt(reader: &mut impl Read, len: usize, reusable_buf: &mut Vec<u8>) -> std::io::Result<String> {
    reusable_buf.resize(len, 0);
    reader.read_exact(reusable_buf)?;
    
    // Como MD5 é 100% ASCII puro, usamos "unsafe" para desligar a verificação de UTF-8 do Rust.
    // Isso economiza centenas de milhões de ciclos de CPU.
    unsafe {
        Ok(String::from_utf8_unchecked(reusable_buf.clone()))
    }
}

#[pg_extern]
fn pg_multimerge_binary_copy_text(query: &str, temp_file_path: &str) -> Result<TableIterator<'static, (name!(val, String),)>, spi::Error> {
    
    let copy_query = format!("COPY ({}) TO '{}' WITH (FORMAT BINARY)", query, temp_file_path);
    Spi::run(&copy_query)?;

    let file = File::open(temp_file_path).expect("Falha ao abrir arquivo binário");
    let mut reader = std::io::BufReader::with_capacity(8 * 1024 * 1024, file);

    let mut magic = [0u8; 11];
    reader.read_exact(&mut magic).unwrap();
    let _flags = read_i32_be(&mut reader).unwrap();
    let ext_len = read_i32_be(&mut reader).unwrap();
    if ext_len > 0 {
        let mut dummy = vec![0u8; ext_len as usize];
        reader.read_exact(&mut dummy).unwrap();
    }

    let mut data: Vec<String> = Vec::with_capacity(10_000_000); 
    
    // O QUE FALTAVA: A declaração do buffer que será reciclado 10 milhões de vezes!
    let mut word_buffer = Vec::with_capacity(512);

    loop {
        let num_fields = read_i16_be(&mut reader).unwrap();
        if num_fields == -1 { break; } // Fim do Arquivo

        let field_len = read_i32_be(&mut reader).unwrap();
        if field_len > 0 {
            // O QUE FALTAVA: Chamar a função _opt passando o word_buffer
            data.push(read_string_be_opt(&mut reader, field_len as usize, &mut word_buffer).unwrap());
        }
    }

    std::fs::remove_file(temp_file_path).ok();

    ordenar_multi_merge_generic(&mut data);

    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}

#[pg_extern]
fn pg_multimerge_stream_i32(query: &str) -> Result<TableIterator<'static, (name!(val, i32),)>, spi::Error> {
    // 1. Conecta via SPI e extrai as tuplas linha a linha
    let mut data: Vec<i32> = Spi::connect(|client| {
        let table = client.select(query, None, &[])?;
        let mut results = Vec::new();
        
        for row in table {
            // Pega o valor da primeira coluna (índice 1 no PostgreSQL)
            if let Some(val) = row.get::<i32>(1)? {
                results.push(val);
            }
        }
        Ok::<Vec<i32>, spi::Error>(results)
    })?;

    // 2. Aplica o seu motor de ordenação multithread
    ordenar_multi_merge_generic(&mut data);

    // 3. Devolve os dados ordenados como um fluxo de tuplas (Table)
    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}

/// Ordenação para BigInt (i64)
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

    ordenar_multi_merge_generic(&mut data);

    Ok(TableIterator::new(data.into_iter().map(|v| (v,))))
}


// ==========================================
// O SEU MOTOR DE ORDENAÇÃO (INTACTO)
// ==========================================

fn calcular_minrun(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 { r |= n & 1; n >>= 1; }
    n + r
}

pub fn insertion_sort<T: Ord>(arr: &mut [T]) {
    for i in 1..arr.len() {
        let mut j = i;
        while j > 0 && arr[j - 1] > arr[j] {
            arr.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn detectar_tendencia_global<T: Ord + Sync>(arr: &mut [T]) -> bool {
    let n = arr.len();
    if n < 100_000 { return false; } 

    let chunk_size = 32768; 
    let qtd_blocos = (n + chunk_size - 1) / chunk_size;
    let arr_imutavel: &[T] = arr;

    let (subindo, descendo) = (0..qtd_blocos)
        .into_par_iter()
        .map(|i| {
            let inicio = i * chunk_size;
            let fim = std::cmp::min(inicio + chunk_size + 1, n);
            let chunk_sobreposto = &arr_imutavel[inicio..fim];
            let mut asc = 0;
            let mut desc = 0;
            for j in 1..chunk_sobreposto.len() {
                if chunk_sobreposto[j - 1] < chunk_sobreposto[j] { asc += 1; }
                else if chunk_sobreposto[j - 1] > chunk_sobreposto[j] { desc += 1; }
            }
            (asc, desc)
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));

    if descendo == 0 { return true; }
    if subindo == 0 { arr.reverse(); return true; }
    if descendo > 0 && descendo < (n / 20) { return false; }
    false
}

pub fn ordenar_multi_merge_generic<T: Ord + Sync + Send + Clone>(arr: &mut [T]) {
    let n = arr.len();
    if n < 1024 {
        insertion_sort(arr);
        return;
    }
    
    if detectar_tendencia_global(arr) { return; }

    let mut e_caos_puro = false;
    if n > 120 {
        let mid = n / 2;
        let mut mudancas_direcao = 0;
        let mut subindo = arr[mid] <= arr[mid + 1];
        
        for i in (mid + 1)..(mid + 100).min(n - 1) {
            let direcao_atual = arr[i] <= arr[i + 1];
            if direcao_atual != subindo {
                mudancas_direcao += 1;
                subindo = direcao_atual;
            }
        }
        if mudancas_direcao > 15 { e_caos_puro = true; }
    }

    if !e_caos_puro {
        let mut buffer = vec![arr[0].clone(); n];
        let num_threads = rayon::current_num_threads();
        let threshold = (n / num_threads).max(1_000_000); 
        sort_recursivo_paralelo(arr, &mut buffer, threshold);
    } else {
        arr.par_sort_unstable();
    }
}

fn ordenar_sequencial_timsort_style<T: Ord + Clone>(arr: &mut [T], buffer: &mut [T]) {
    let n = arr.len();
    let minrun = calcular_minrun(n);

    for i in (0..n).step_by(minrun) {
        let end = (i + minrun).min(n);
        insertion_sort(&mut arr[i..end]);
    }

    let mut tamanho_bloco = minrun;
    while tamanho_bloco < n {
        for esq in (0..n).step_by(tamanho_bloco * 2) {
            let meio = (esq + tamanho_bloco).min(n);
            let dir = (esq + tamanho_bloco * 2).min(n);
            if meio < dir {
                mesclar_estavel(&mut arr[esq..dir], buffer, meio - esq);
            }
        }
        tamanho_bloco *= 2;
    }
}

fn sort_recursivo_paralelo<T: Ord + Clone + Send>(arr: &mut [T], buffer: &mut [T], threshold: usize) {
    let n = arr.len();
    if n <= threshold {
        ordenar_sequencial_timsort_style(arr, buffer);
        return;
    }

    let meio = n / 2;
    let (arr_esq, arr_dir) = arr.split_at_mut(meio);
    let (buf_esq, buf_dir) = buffer.split_at_mut(meio);

    rayon::join(
        || sort_recursivo_paralelo(arr_esq, buf_esq, threshold),
        || sort_recursivo_paralelo(arr_dir, buf_dir, threshold),
    );

    mesclar_estavel(arr, buffer, meio);
}

fn mesclar_estavel<T: Ord + Clone>(arr: &mut [T], buffer: &mut [T], meio: usize) {
    let n = arr.len();
    buffer[..n].clone_from_slice(&arr[..n]);

    let mut i = 0;
    let mut j = meio;
    let mut k = 0;

    while i < meio && j < n {
        if buffer[i] <= buffer[j] { arr[k] = buffer[i].clone(); i += 1; }
        else { arr[k] = buffer[j].clone(); j += 1; }
        k += 1;
    }

    if i < meio { arr[k..k + (meio - i)].clone_from_slice(&buffer[i..meio]); }
    else if j < n { arr[k..k + (n - j)].clone_from_slice(&buffer[j..n]); }
}