import init, { FeeExplorer } from './pkg/fee_explorer.js';

class BitcoinFeeExplorer {
    constructor() {
        this.wasmModule = null;
        this.explorer = null;
        this.metadata = null;
        this.arrowData = new Map();
        this.leftAxisMetrics = []; // Array of metric objects for left axis
        this.rightAxisMetrics = []; // Array of metric objects for right axis
        this.chart = null;
        this.currentModalAxis = null; // 'left' or 'right'
        this.nextColorIndex = 0; // For assigning colors to new metrics
        this.colorPalette = ['#1f77b4', '#ff7f0e', '#2ca02c', '#d62728', '#9467bd', '#8c564b', '#e377c2', '#7f7f7f', '#bcbd22', '#17becf'];
    }

    async initialize() {
        try {
            this.updateStatus('Loading WASM module...');
            console.log('Starting WASM initialization...');

            // Initialize WASM
            console.log('Calling init()...');
            await init();
            console.log('WASM init() completed successfully');

            console.log('Creating FeeExplorer instance...');
            this.explorer = new FeeExplorer();
            console.log('FeeExplorer created successfully');

            this.updateStatus('Loading metadata...');
            await this.loadMetadata();

            this.updateStatus('Loading datasets...');
            await this.loadDatasets();

            this.setupUI();
            this.updateStatus('Ready! Select metrics and update chart.');

        } catch (error) {
            console.error('Initialization failed:', error);
            this.updateStatus(`Error: ${error.message}`);
        }
    }

    async loadMetadata() {
        const response = await fetch('./data/datasets.json');
        if (!response.ok) {
            throw new Error(`Failed to load metadata: ${response.statusText}`);
        }

        const metadataText = await response.text();
        this.metadata = JSON.parse(metadataText);

        // Don't load into WASM yet - we'll update the range first after loading Arrow files
    }

    async loadDatasets() {
        // Load real Arrow files
        await this.loadArrowDatasets();
    }

    async loadArrowDatasets() {
        console.log('Loading real Arrow datasets...');

        // Check if Arrow library is loaded
        if (typeof Arrow === 'undefined') {
            throw new Error('Apache Arrow library not loaded. Check if the CDN is accessible.');
        }

        console.log('Arrow library available:', typeof Arrow);
        this.arrowData = new Map();
        let minHeight = Infinity;
        let maxHeight = -Infinity;

        // Load each Arrow file from the metadata
        const totalDatasets = this.metadata.datasets.length;
        this.updateProgress(0, true); // Show progress bar at 0%

        for (let i = 0; i < this.metadata.datasets.length; i++) {
            const dataset = this.metadata.datasets[i];
            try {
                // Update status and progress for current dataset
                this.updateStatus(`Loading dataset ${i + 1} of ${totalDatasets}: ${dataset.name}`);
                console.log(`Loading ${dataset.file}...`);

                // Step 1: Initiate download
                const baseProgress = (i / totalDatasets) * 100;
                const progressPerDataset = 100 / totalDatasets;
                this.updateProgress(baseProgress);

                const response = await fetch(`./data/${dataset.file}`);
                console.log(`Response status for ${dataset.file}:`, response.status, response.statusText);

                if (!response.ok) {
                    throw new Error(`Failed to load ${dataset.file}: ${response.status} ${response.statusText}`);
                }

                // Step 2: Download with progress tracking
                const contentLength = response.headers.get('Content-Length');
                const totalBytes = contentLength ? parseInt(contentLength) : null;
                console.log(`${dataset.file} size: ${totalBytes ? (totalBytes / 1024 / 1024).toFixed(1) + ' MB' : 'unknown size'}`);

                let downloadedBytes = 0;
                const chunks = [];
                const reader = response.body.getReader();

                while (true) {
                    const { done, value } = await reader.read();
                    if (done) break;

                    chunks.push(value);
                    downloadedBytes += value.length;

                    // Update progress during download (use 80% of the dataset's progress allocation for download)
                    if (totalBytes) {
                        const downloadProgress = (downloadedBytes / totalBytes) * 0.8; // 80% of this dataset's progress
                        this.updateProgress(baseProgress + (downloadProgress * progressPerDataset));
                        this.updateStatus(`Downloading ${dataset.name}: ${(downloadedBytes / 1024 / 1024).toFixed(1)} / ${(totalBytes / 1024 / 1024).toFixed(1)} MB`);
                    } else {
                        this.updateStatus(`Downloading ${dataset.name}: ${(downloadedBytes / 1024 / 1024).toFixed(1)} MB`);
                    }
                }

                // Combine chunks into ArrayBuffer
                const arrayBuffer = new Uint8Array(downloadedBytes);
                let offset = 0;
                for (const chunk of chunks) {
                    arrayBuffer.set(chunk, offset);
                    offset += chunk.length;
                }

                console.log(`${dataset.file} downloaded: ${arrayBuffer.byteLength} bytes`);

                // Step 3: Parse Arrow data (use next 15% of progress)
                this.updateProgress(baseProgress + (0.8 * progressPerDataset));
                this.updateStatus(`Parsing ${dataset.name}...`);

                // Try different Arrow API methods depending on version
                let table;
                try {
                    console.log(`Attempting to parse ${dataset.file} with tableFromIPC...`);
                    // Try the newer API first
                    table = Arrow.tableFromIPC(arrayBuffer.buffer);
                    console.log(`Successfully parsed ${dataset.file} with tableFromIPC`);
                } catch (e1) {
                    console.log(`tableFromIPC failed for ${dataset.file}:`, e1.message);
                    try {
                        console.log(`Attempting ${dataset.file} with Table.from...`);
                        // Try alternative API
                        table = Arrow.Table.from([Arrow.RecordBatch.from(arrayBuffer.buffer)]);
                        console.log(`Successfully parsed ${dataset.file} with Table.from`);
                    } catch (e2) {
                        console.log(`Table.from failed for ${dataset.file}:`, e2.message);
                        try {
                            console.log(`Attempting ${dataset.file} with RecordBatchFileReader...`);
                            // Try reading as IPC file
                            const reader = Arrow.RecordBatchFileReader.from(arrayBuffer.buffer);
                            table = new Arrow.Table(reader.readAll());
                            console.log(`Successfully parsed ${dataset.file} with RecordBatchFileReader`);
                        } catch (e3) {
                            console.log('Arrow API attempts failed for', dataset.file, ':', {e1: e1.message, e2: e2.message, e3: e3.message});
                            console.log('Available Arrow methods:', Object.keys(Arrow));
                            throw new Error(`Unable to parse ${dataset.file} with any known API method`);
                        }
                    }
                }

                console.log(`Loaded ${dataset.file}: ${table.numRows} rows, ${table.numCols} columns`);
                console.log('Columns:', table.schema.fields.map(f => f.name));

                // Step 4: Process height data (use final 5% of progress)
                this.updateProgress(baseProgress + (0.95 * progressPerDataset));
                this.updateStatus(`Processing ${dataset.name}...`);

                // Find min/max heights from this dataset
                try {
                    const heightColumn = table.getChild('height');
                    if (heightColumn && table.numRows > 0) {
                        console.log(`Getting height data from ${dataset.file}...`);
                        const heights = heightColumn.toArray();
                        console.log(`Height array length: ${heights.length}, first few values:`, heights.slice(0, 5));

                        // Use regular loop instead of spread operator to avoid "too many arguments" error
                        let datasetMin = heights[0];
                        let datasetMax = heights[0];
                        for (let i = 1; i < heights.length; i++) {
                            if (heights[i] < datasetMin) datasetMin = heights[i];
                            if (heights[i] > datasetMax) datasetMax = heights[i];
                        }

                        minHeight = Math.min(minHeight, datasetMin);
                        maxHeight = Math.max(maxHeight, datasetMax);
                        console.log(`Dataset ${dataset.file} height range: ${datasetMin} to ${datasetMax}`);
                    }
                } catch (heightError) {
                    console.error(`Error processing height data for ${dataset.file}:`, heightError);
                    throw heightError;
                }

                // Validate that all columns in datasets.json actually exist in the Arrow file
                const actualColumns = new Set(table.schema.fields.map(field => field.name));
                const expectedColumns = Object.keys(dataset.columns);
                const missingColumns = expectedColumns.filter(col => !actualColumns.has(col));

                if (missingColumns.length > 0) {
                    const error = `Dataset validation failed for ${dataset.file}. Missing columns: ${missingColumns.join(', ')}. Available columns: ${Array.from(actualColumns).join(', ')}`;
                    console.error(error);
                    throw new Error(error);
                }

                this.arrowData.set(dataset.name, {
                    table: table,
                    dataset: dataset
                });

                // Complete this dataset
                this.updateProgress(((i + 1) / totalDatasets) * 100);

            } catch (error) {
                console.error(`Failed to load ${dataset.file}:`, error);
                throw new Error(`Could not load ${dataset.file}. Make sure you've exported the Arrow file first.`);
            }
        }

        // Update metadata with discovered block range
        if (minHeight !== Infinity && maxHeight !== -Infinity) {
            this.metadata.block_range = {
                start: minHeight,
                end: maxHeight
            };
            console.log(`Discovered block range: ${minHeight} to ${maxHeight}`);

            // Update the data range display
            this.updateDataRangeDisplay(maxHeight);
        } else {
            console.warn('No height data found in Arrow files, keeping default range');
            document.getElementById('dataRange').textContent = 'No data available';
        }

        // Now load the updated metadata into WASM
        console.log('Loading metadata into WASM...');
        const updatedMetadata = JSON.stringify(this.metadata);
        console.log('Metadata to load:', updatedMetadata);
        await this.explorer.load_metadata(updatedMetadata);
        console.log('Metadata loaded into WASM successfully');

        // Hide progress bar and show completion
        this.updateProgress(100, false);
        this.updateDataStatus(`Loaded ${this.arrowData.size} Arrow datasets successfully`);
    }

    setupUI() {
        this.populateMetricSelect();
        this.setupEventListeners();
        this.initializeChart();

        // Hide loading headers after UI is fully set up with animation
        this.hideLoadingHeaders();
    }

    hideLoadingHeaders() {
        // Pause for 1 second to let users see the final message, then animate
        setTimeout(() => {
            const headerMobile = document.getElementById('headerOverlayMobile');
            const headerDesktop = document.getElementById('headerDesktop');

            if (headerMobile) {
                headerMobile.classList.add('slide-up');
                // Remove from DOM after animation completes
                setTimeout(() => {
                    headerMobile.style.display = 'none';
                }, 600);
            }

            if (headerDesktop) {
                // Keep desktop header visible, just hide loading elements
                const loadingStatus = headerDesktop.querySelector('.loading-status');
                const progressContainer = headerDesktop.querySelector('.progress-container');

                if (loadingStatus) loadingStatus.style.display = 'none';
                if (progressContainer) progressContainer.style.display = 'none';
            }

            // Show tutorial after headers are hidden
            setTimeout(() => {
                this.showTutorial();
            }, 700); // Show tutorial after header animation completes
        }, 1000); // 1 second pause before animation starts
    }

    showTutorial() {
        const tutorialOverlay = document.getElementById('tutorialOverlay');
        if (tutorialOverlay) {
            tutorialOverlay.style.display = 'flex';

            // Click anywhere to dismiss
            tutorialOverlay.addEventListener('click', () => this.hideTutorial());

            // Also set up button click handler (redundant but explicit)
            const dismissButton = document.getElementById('tutorialDismiss');
            if (dismissButton) {
                dismissButton.addEventListener('click', () => this.hideTutorial());
            }
        }
    }

    hideTutorial() {
        const tutorialOverlay = document.getElementById('tutorialOverlay');
        if (tutorialOverlay) {
            tutorialOverlay.style.display = 'none';
        }
    }

    populateMetricSelect() {
        console.log('Populating metric select...');
        const selectMobile = document.getElementById('metricSelectMobile');
        const selectDesktop = document.getElementById('metricSelectDesktop');

        if (selectMobile) selectMobile.innerHTML = '';
        if (selectDesktop) selectDesktop.innerHTML = '';

        try {
            // Get available metrics from WASM module
            console.log('Calling get_available_metrics()...');
            const metrics = this.explorer.get_available_metrics();
            console.log('Got metrics from WASM:', metrics);
            console.log('Number of metrics:', metrics ? metrics.length : 'null/undefined');

            if (!metrics || metrics.length === 0) {
                console.warn('No metrics returned from WASM module');
                if (selectMobile) selectMobile.innerHTML = '<option>No metrics available</option>';
                if (selectDesktop) selectDesktop.innerHTML = '<option>No metrics available</option>';
                return;
            }

            // Sort metrics alphabetically by name
            const sortedMetrics = [...metrics].sort((a, b) => a.name.localeCompare(b.name));

            sortedMetrics.forEach((metric, index) => {
                console.log(`Processing metric ${index}:`, metric);

                // Create option for mobile
                if (selectMobile) {
                    const optionMobile = document.createElement('option');
                    optionMobile.value = `${metric.dataset}::${metric.name}`;
                    optionMobile.textContent = `${metric.name} (${metric.unit}) - ${metric.description}`;
                    optionMobile.dataset.unit = metric.unit;
                    optionMobile.dataset.dataset = metric.dataset;
                    optionMobile.dataset.metricName = metric.name;
                    selectMobile.appendChild(optionMobile);
                }

                // Create option for desktop
                if (selectDesktop) {
                    const optionDesktop = document.createElement('option');
                    optionDesktop.value = `${metric.dataset}::${metric.name}`;
                    optionDesktop.textContent = `${metric.name} (${metric.unit}) - ${metric.description}`;
                    optionDesktop.dataset.unit = metric.unit;
                    optionDesktop.dataset.dataset = metric.dataset;
                    optionDesktop.dataset.metricName = metric.name;
                    selectDesktop.appendChild(optionDesktop);
                }
            });

            console.log('Metric select populated successfully');
        } catch (error) {
            console.error('Error populating metrics:', error);
            if (selectMobile) selectMobile.innerHTML = '<option>Error loading metrics</option>';
            if (selectDesktop) selectDesktop.innerHTML = '<option>Error loading metrics</option>';
        }
    }

    setupEventListeners() {
        // Axis management buttons
        const leftAxisButton = document.getElementById('leftAxisButton');
        const rightAxisButton = document.getElementById('rightAxisButton');

        if (leftAxisButton) {
            leftAxisButton.addEventListener('click', () => this.openMetricModal('left'));
        }
        if (rightAxisButton) {
            rightAxisButton.addEventListener('click', () => this.openMetricModal('right'));
        }

        // Modal event listeners
        const modalClose = document.getElementById('modalClose');
        const metricModal = document.getElementById('metricModal');

        if (modalClose) {
            modalClose.addEventListener('click', () => this.closeMetricModal());
        }

        if (metricModal) {
            metricModal.addEventListener('click', (e) => {
                if (e.target === metricModal) {
                    this.closeMetricModal();
                }
            });
        }

        // Handle window resize for responsive chart height
        window.addEventListener('resize', () => {
            if (this.chart && this.chart.data && this.chart.data.length > 0) {
                this.updateChartLayout();
            }
        });
    }

    initializeChart() {
        const chartDiv = document.getElementById('mainChart');

        // Responsive height: full screen on mobile, fixed on desktop
        const isMobile = window.innerWidth <= 767;
        const chartHeight = isMobile ? window.innerHeight : 600;

        const layout = {
            title: '',
            height: chartHeight,
            xaxis: {
                title: 'Block Height',
                type: 'linear'
            },
            yaxis: {
                title: 'Left Axis',
                side: 'left',
                overlaying: false
            },
            yaxis2: {
                title: 'Right Axis',
                side: 'right',
                overlaying: 'y'
            },
            legend: {
                x: 0,
                y: 1,
                bgcolor: 'rgba(255,255,255,0.8)'
            },
            hovermode: 'x unified',
            dragmode: 'zoom' // Try zoom mode for both mobile and desktop
        };

        const config = isMobile ? {
            // Clean mobile configuration
            responsive: true,
            displayModeBar: false,  // Hide mode bar for clean mobile interface
            scrollZoom: true,
            doubleClick: 'reset+autosize'
        } : {
            // Desktop configuration
            responsive: true,
            displayModeBar: true,
            modeBarButtonsToRemove: ['zoom2d', 'pan2d', 'zoomIn2d', 'zoomOut2d', 'resetScale2d'],
            scrollZoom: true,
            doubleClick: 'reset+autosize'
        };

        Plotly.newPlot(chartDiv, [], layout, config);
        this.chart = chartDiv;
    }

    async updateChart() {
        this.updateStatus('Updating chart...');

        try {
            const startHeight = this.metadata.block_range.start;
            const endHeight = this.metadata.block_range.end;
            const traces = [];

            // Create traces for left axis metrics
            this.createAxisTraces(traces, this.leftAxisMetrics, 'y', startHeight, endHeight);

            // Create traces for right axis metrics
            this.createAxisTraces(traces, this.rightAxisMetrics, 'y2', startHeight, endHeight);

            await Plotly.react(this.chart, traces, this.getChartLayout());
            this.updateStatus('Chart updated successfully');

        } catch (error) {
            console.error('Chart update failed:', error);
            this.updateStatus(`Chart update failed: ${error.message}`);
        }
    }

    createAxisTraces(traces, axisMetrics, yaxis, startHeight, endHeight) {
        for (const metric of axisMetrics) {
            // Find the specific dataset for this metric
            const datasetEntry = this.arrowData.get(metric.dataset);
            if (!datasetEntry) {
                console.warn(`Dataset '${metric.dataset}' not found for metric '${metric.name}'`);
                continue;
            }

            const {table, dataset} = datasetEntry;

            // Check if this metric exists in this dataset
            if (!table.schema.fields.find(f => f.name === metric.name)) {
                console.warn(`Metric '${metric.name}' not found in dataset '${metric.dataset}'`);
                continue;
            }

            // Get height column
            const heightColumn = table.getChild('height');
            const heights = heightColumn.toArray();

            // Filter data by height range
            const indices = [];
            for (let i = 0; i < heights.length; i++) {
                const height = heights[i];
                if (height >= startHeight && height <= endHeight) {
                    indices.push(i);
                }
            }

            // Get the metric column
            const column = table.getChild(metric.name);

            // Extract filtered data
            const filteredHeights = indices.map(i => heights[i]);
            const filteredValues = indices.map(i => column.get(i));

            // Create main trace
            const traceName = metric.maWindow ?
                `${metric.name} (${metric.maWindow}-block MA)` :
                metric.name;

            let finalValues = filteredValues;
            let finalHeights = filteredHeights;

            // Apply moving average if specified
            if (metric.maWindow && filteredValues.length > metric.maWindow) {
                finalValues = this.calculateMovingAverage(filteredValues, metric.maWindow);
                finalHeights = filteredHeights.slice(metric.maWindow - 1);
            }

            const trace = {
                x: finalHeights,
                y: finalValues,
                name: traceName,
                type: 'scatter',
                mode: 'lines',
                yaxis: yaxis,
                line: {
                    width: 1.5,
                    color: metric.color
                }
            };

            traces.push(trace);
        }
    }

    createMockTraces(traces, metricNames, startHeight, endHeight, showMA, maWindow) {
        // Generate mock data for demonstration
        const numPoints = Math.min(1000, endHeight - startHeight + 1);
        const heights = Array.from({length: numPoints}, (_, i) =>
            startHeight + Math.floor(i * (endHeight - startHeight) / (numPoints - 1))
        );

        metricNames.forEach((metricName, index) => {
            const metric = Array.from(this.selectedMetrics).find(m => m.name === metricName);
            const yaxis = this.assignYAxis(metric.unit);

            // Generate realistic mock data based on metric type
            const values = heights.map(h => this.generateMockValue(metricName, h));

            traces.push({
                x: heights,
                y: values,
                type: 'scatter',
                mode: 'lines',
                name: `${metricName} (${metric.unit})`,
                yaxis: yaxis,
                line: { width: 2 },
                hovertemplate: `<b>${metricName}</b><br>Block: %{x}<br>Value: %{y:.2f} ${metric.unit}<extra></extra>`
            });

            // Add moving average if requested
            if (showMA && values.length >= maWindow) {
                const maValues = this.calculateMovingAverage(values, maWindow);
                const maHeights = heights.slice(maWindow - 1);

                traces.push({
                    x: maHeights,
                    y: maValues,
                    type: 'scatter',
                    mode: 'lines',
                    name: `${maWindow}MA ${metricName}`,
                    yaxis: yaxis,
                    line: { width: 3, dash: 'dash' },
                    opacity: 0.7
                });
            }
        });
    }

    generateMockValue(metricName, height) {
        // Generate realistic mock data patterns
        const baseHeight = 700000;
        const progress = (height - baseHeight) / 100000;

        switch (metricName) {
            case 'tx_count':
                return 1500 + Math.sin(progress * 10) * 500 + Math.random() * 300;
            case 'fee_avg':
                return 10 + Math.sin(progress * 5) * 8 + Math.random() * 5;
            case 'fee_min':
                return 1 + Math.random() * 2;
            case 'fee_max':
                return 100 + Math.sin(progress * 3) * 200 + Math.random() * 100;
            case 'block_size':
                return 1000000 + Math.sin(progress * 7) * 300000 + Math.random() * 100000;
            case 'sub_1sat_count':
                return Math.floor(Math.random() * 50);
            case 'op_return_max_size':
                return 40 + Math.floor(Math.random() * 40);
            case 'difficulty':
                return 20000000000000 + progress * 10000000000000 + Math.random() * 1000000000000;
            default:
                return Math.random() * 100;
        }
    }

    assignYAxis(unit) {
        // Assign metrics to left or right y-axis based on units
        const leftAxisUnits = ['transactions', 'sat/vB', 'bytes'];
        const rightAxisUnits = ['difficulty', 'EH/s'];

        if (leftAxisUnits.includes(unit)) {
            return 'y';
        } else if (rightAxisUnits.includes(unit)) {
            return 'y2';
        } else {
            // Default assignment based on typical ranges
            return 'y';
        }
    }

    calculateMovingAverage(values, window) {
        const ma = [];
        if (values.length < window) return ma;

        // Calculate initial sum for first window
        let sum = 0;
        for (let i = 0; i < window; i++) {
            sum += values[i];
        }
        ma.push(sum / window);

        // Use sliding window: remove first element, add next element
        for (let i = window; i < values.length; i++) {
            sum = sum - values[i - window] + values[i];
            ma.push(sum / window);
        }

        return ma;
    }

    getChartLayout() {
        // Responsive height: full screen on mobile, fixed on desktop
        const isMobile = window.innerWidth <= 767;
        const chartHeight = isMobile ? window.innerHeight : 600;

        // Generate axis titles based on current metrics
        const leftAxisTitle = this.leftAxisMetrics.length > 0 ?
            `Left Axis (${this.leftAxisMetrics[0].unit})` : 'Left Axis';
        const rightAxisTitle = this.rightAxisMetrics.length > 0 ?
            `Right Axis (${this.rightAxisMetrics[0].unit})` : 'Right Axis';

        return {
            height: chartHeight,
            margin: {
                t: 50,  // Reduced top margin to minimize gap
                r: 50,
                b: 50,
                l: 50
            },
            xaxis: {
                title: 'Block Height',
                type: 'linear',
                showgrid: true
            },
            yaxis: {
                title: leftAxisTitle,
                side: 'left',
                type: 'linear',
                showgrid: true
            },
            yaxis2: {
                title: rightAxisTitle,
                side: 'right',
                overlaying: 'y',
                type: 'linear'
            },
            legend: {
                x: 0.02,
                y: 0.98,
                bgcolor: 'rgba(255,255,255,0.8)',
                bordercolor: 'rgba(0,0,0,0.2)',
                borderwidth: 1
            },
            showlegend: true,
            hovermode: 'x unified',
            dragmode: 'zoom'
        };
    }

    updateChartLayout() {
        if (this.chart && this.chart.data && this.chart.data.length > 0) {
            Plotly.relayout(this.chart, this.getChartLayout());
        }
    }

    resetZoom() {
        if (this.chart) {
            Plotly.relayout(this.chart, {
                'xaxis.autorange': true,
                'yaxis.autorange': true,
                'yaxis2.autorange': true
            });
        }
    }

    updateStatus(message) {
        const loadingStatusMobile = document.getElementById('loadingStatusMobile');
        const loadingStatusDesktop = document.getElementById('loadingStatusDesktop');

        if (loadingStatusMobile) loadingStatusMobile.textContent = message;
        if (loadingStatusDesktop) loadingStatusDesktop.textContent = message;
    }

    updateProgress(percentage, show = true) {
        const progressContainerMobile = document.getElementById('progressContainerMobile');
        const progressContainerDesktop = document.getElementById('progressContainerDesktop');
        const progressBarMobile = document.getElementById('progressBarMobile');
        const progressBarDesktop = document.getElementById('progressBarDesktop');

        if (show) {
            if (progressContainerMobile) progressContainerMobile.style.display = 'block';
            if (progressContainerDesktop) progressContainerDesktop.style.display = 'block';
        } else {
            if (progressContainerMobile) progressContainerMobile.style.display = 'none';
            if (progressContainerDesktop) progressContainerDesktop.style.display = 'none';
        }

        if (progressBarMobile) progressBarMobile.style.width = `${percentage}%`;
        if (progressBarDesktop) progressBarDesktop.style.width = `${percentage}%`;
    }

    updateDataStatus(message) {
        const dataStatusMobile = document.getElementById('dataStatusMobile');
        const dataStatusDesktop = document.getElementById('dataStatusDesktop');
        if (dataStatusMobile) dataStatusMobile.textContent = message;
        if (dataStatusDesktop) dataStatusDesktop.textContent = message;
    }

    updateDataRangeDisplay(maxHeight) {
        // Find the timestamp for the highest block height
        let maxTimestamp = null;

        for (const [datasetName, {table, dataset}] of this.arrowData) {
            const heightColumn = table.getChild('height');
            const timestampColumn = table.getChild('timestamp');

            if (heightColumn && timestampColumn) {
                const heights = heightColumn.toArray();
                const timestamps = timestampColumn.toArray();

                for (let i = 0; i < heights.length; i++) {
                    if (heights[i] === maxHeight) {
                        maxTimestamp = timestamps[i];
                        break;
                    }
                }

                if (maxTimestamp !== null) break;
            }
        }

        // Format the display message
        let message = `Latest block: ${maxHeight}`;
        if (maxTimestamp !== null) {
            const date = new Date(maxTimestamp * 1000); // Convert Unix timestamp to Date
            message += ` (${date.toLocaleDateString()} ${date.toLocaleTimeString()})`;
        }

        document.getElementById('dataRange').textContent = message;
    }

    // Modal management methods
    openMetricModal(axis) {
        this.currentModalAxis = axis;
        const modal = document.getElementById('metricModal');
        const modalTitle = document.getElementById('modalTitle');
        const maDropdown = document.getElementById('maDropdown');

        modalTitle.textContent = `Manage ${axis.charAt(0).toUpperCase() + axis.slice(1)} Axis`;

        // Reset MA dropdown to "None"
        if (maDropdown) {
            maDropdown.value = 'none';
        }

        modal.style.display = 'flex';

        this.populateModal();
    }

    closeMetricModal() {
        const modal = document.getElementById('metricModal');
        modal.style.display = 'none';
        this.currentModalAxis = null;
    }

    populateModal() {
        this.populateCurrentMetrics();
        this.populateAvailableMetrics();
    }

    populateCurrentMetrics() {
        const currentMetricsList = document.getElementById('currentMetricsList');
        const metrics = this.currentModalAxis === 'left' ? this.leftAxisMetrics : this.rightAxisMetrics;

        currentMetricsList.innerHTML = '';

        if (metrics.length === 0) {
            currentMetricsList.innerHTML = '<div style="color: #999; font-style: italic; padding: 20px;">No metrics selected</div>';
            return;
        }

        metrics.forEach((metric, index) => {
            const metricItem = document.createElement('div');
            metricItem.className = 'current-metric-item';
            metricItem.style.borderColor = metric.color;
            metricItem.style.color = metric.color;

            const metricText = metric.maWindow ?
                `${metric.name} (${metric.maWindow}-block MA)` :
                metric.name;

            const removeButton = document.createElement('button');
            removeButton.className = 'current-metric-remove';
            removeButton.textContent = '×';
            removeButton.addEventListener('click', () => this.removeMetric(this.currentModalAxis, index));

            metricItem.innerHTML = metricText;
            metricItem.appendChild(removeButton);

            currentMetricsList.appendChild(metricItem);
        });
    }

    populateAvailableMetrics() {
        const availableMetricsList = document.getElementById('availableMetricsList');
        const currentMetrics = this.currentModalAxis === 'left' ? this.leftAxisMetrics : this.rightAxisMetrics;

        availableMetricsList.innerHTML = '';

        // Get the current unit for this axis (if any metrics exist)
        const currentUnit = currentMetrics.length > 0 ? currentMetrics[0].unit : null;

        // Get all available metrics from WASM and sort them alphabetically
        const allMetrics = this.explorer.get_available_metrics();
        const sortedMetrics = allMetrics.sort((a, b) => a.name.localeCompare(b.name));

        sortedMetrics.forEach(metric => {
            const isCompatible = !currentUnit || metric.unit === currentUnit;

            const metricItem = document.createElement('div');
            metricItem.className = `available-metric-item ${isCompatible ? '' : 'disabled'}`;
            metricItem.textContent = `${metric.name} (${metric.unit}) - ${metric.description}`;

            if (isCompatible) {
                metricItem.addEventListener('click', () => this.addMetric(metric));
            }

            availableMetricsList.appendChild(metricItem);
        });
    }

    addMetric(metricInfo) {
        const maDropdown = document.getElementById('maDropdown');
        const maWindow = maDropdown.value !== 'none' ? parseInt(maDropdown.value) : null;

        const newMetric = {
            uniqueId: `${metricInfo.dataset}::${metricInfo.name}`,
            name: metricInfo.name,
            unit: metricInfo.unit,
            dataset: metricInfo.dataset,
            maWindow: maWindow,
            color: this.colorPalette[this.nextColorIndex % this.colorPalette.length]
        };

        this.nextColorIndex++;

        if (this.currentModalAxis === 'left') {
            this.leftAxisMetrics.push(newMetric);
        } else {
            this.rightAxisMetrics.push(newMetric);
        }

        // Close modal and update chart
        this.closeMetricModal();
        this.updateChart();
    }

    removeMetric(axis, index) {
        if (axis === 'left') {
            this.leftAxisMetrics.splice(index, 1);
        } else {
            this.rightAxisMetrics.splice(index, 1);
        }

        // Close modal and update chart
        this.closeMetricModal();
        this.updateChart();
    }
}

// Initialize the application
const app = new BitcoinFeeExplorer();
app.initialize();
