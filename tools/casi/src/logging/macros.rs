#[macro_export]
macro_rules! operation {
    ($operation: literal ($($field: ident = $value: expr),*) $fn_name: ident ($self: ident, $($arg_n: expr),*)) => {{
        use tracing::Instrument;

        async move {
            let span = tracing::Span::current();
            let ctx = $crate::logging::TraceContext::new($operation).await;
            let mut metrics = $crate::logging::PerformanceMetrics::new();
            ctx.record(&span);
            tracing::info!("{} started", $operation);
            match $self
                .$fn_name(&mut metrics, $($arg_n),*)
                .await {
                Ok(result) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::info!(
                        "{} completed successfully",
                        $operation,
                    );
                    Ok(result)
                },
                Err(e) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::error!(
                        "{} failed: {}",
                        $operation,
                        e
                    );
                    Err(e)
                }
            }
        }.instrument(tracing::info_span!(
                $operation,
                operation = $operation,
                session_id = tracing::field::Empty,
                correlation_id = tracing::field::Empty,
                artifact_id = tracing::field::Empty,
                duration_ms = tracing::field::Empty,
                bytes_processed = tracing::field::Empty,
                files_processed = tracing::field::Empty,
                throughput_bps = tracing::field::Empty,
                cache_hit_ratio = tracing::field::Empty,
                network_requests = tracing::field::Empty,
                network_bytes = tracing::field::Empty,
                $($field = $value),*
            ))
            .await
    }};

    ($operation: literal ($($field: ident = $value: expr),*) $fn_name: ident ($self: ident, $($arg_n: expr),*) where artifact_id = $artifact_id: expr) => {{
        use tracing::Instrument;

        async move {
            let span = tracing::Span::current();
            let ctx = $crate::logging::TraceContext::new($operation).await;
            let mut metrics = $crate::logging::PerformanceMetrics::new();
            ctx.record(&span);
            tracing::info!("{} started", $operation);
            match $self
                .$fn_name(&mut metrics, $($arg_n),*)
                .await {
                Ok(result) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::info!(
                        "{} completed successfully",
                        $operation,
                    );
                    Ok(result)
                },
                Err(e) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::error!(
                        "{} failed: {}",
                        $operation,
                        e
                    );
                    Err(e)
                }
            }
        }.instrument(tracing::info_span!(
                $operation,
                operation = $operation,
                session_id = tracing::field::Empty,
                correlation_id = tracing::field::Empty,
                artifact_id = $artifact_id,
                duration_ms = tracing::field::Empty,
                bytes_processed = tracing::field::Empty,
                files_processed = tracing::field::Empty,
                throughput_bps = tracing::field::Empty,
                cache_hit_ratio = tracing::field::Empty,
                network_requests = tracing::field::Empty,
                network_bytes = tracing::field::Empty,
                $($field = $value),*
            ))
            .await
    }};
}

#[macro_export]
macro_rules! inline_operation {
    ($operation: literal ($($field: ident = $value: expr),*) $fn: ident) => {{
        use tracing::Instrument;

        async move {
            let span = tracing::Span::current();
            let ctx = $crate::logging::TraceContext::new($operation).await;
            let mut metrics = $crate::logging::PerformanceMetrics::new();
            ctx.record(&span);
            tracing::info!("{} started", $operation);
            match $fn(&mut metrics)
                .await {
                Ok(result) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::info!(
                        "{} completed successfully",
                        $operation,
                    );
                    Ok(result)
                },
                Err(e) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::error!(
                        "{} failed: {}",
                        $operation,
                        e
                    );
                    Err(e)
                }
            }
        }.instrument(tracing::info_span!(
                $operation,
                operation = $operation,
                session_id = tracing::field::Empty,
                correlation_id = tracing::field::Empty,
                artifact_id = tracing::field::Empty,
                duration_ms = tracing::field::Empty,
                bytes_processed = tracing::field::Empty,
                files_processed = tracing::field::Empty,
                throughput_bps = tracing::field::Empty,
                cache_hit_ratio = tracing::field::Empty,
                network_requests = tracing::field::Empty,
                network_bytes = tracing::field::Empty,
                $($field = $value),*
            ))
    }};

    ($operation: literal ($($field: ident = $value: expr),*) $fn: ident; where artifact_id = $artifact_id: expr) => {{
        use tracing::Instrument;

        async move {
            let span = tracing::Span::current();
            let ctx = $crate::logging::TraceContext::new($operation).await;
            let mut metrics = $crate::logging::PerformanceMetrics::new();
            ctx.record(&span);
            tracing::info!("{} started", $operation);
            match $fn(&mut metrics)
                .await {
                Ok(result) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::info!(
                        "{} completed successfully",
                        $operation,
                    );
                    Ok(result)
                },
                Err(e) => {
                    metrics.complete();
                    metrics.record(&span);
                    tracing::error!(
                        "{} failed: {}",
                        $operation,
                        e
                    );
                    Err(e)
                }
            }
        }.instrument(tracing::info_span!(
                $operation,
                operation = $operation,
                session_id = tracing::field::Empty,
                correlation_id = tracing::field::Empty,
                artifact_id = $artifact_id,
                duration_ms = tracing::field::Empty,
                bytes_processed = tracing::field::Empty,
                files_processed = tracing::field::Empty,
                throughput_bps = tracing::field::Empty,
                cache_hit_ratio = tracing::field::Empty,
                network_requests = tracing::field::Empty,
                network_bytes = tracing::field::Empty,
                $($field = $value),*
            ))
    }};
}
